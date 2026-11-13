use axum::{Json, extract::Query, extract::State};
use rillfp::Validation;
use rss_centr_core::semantic::{
    ClaimEmbeddingsResponse, StoreEmbeddingsRequest, StoreEmbeddingsResult,
};
use serde::Deserialize;
use tracing::{debug, instrument};

use crate::search::SearchState;
use crate::search::embedding_store::{claim_items_needing_embedding, store_item_embeddings};

use super::error::HandlerError;

const MAX_CLAIM_LIMIT: i64 = 256;

#[derive(Debug, Deserialize)]
pub struct ClaimEmbeddingsQuery {
    pub limit: Option<i64>,
    pub lease_seconds: Option<i64>,
}

#[derive(Debug, PartialEq, Eq)]
struct ClaimParameters {
    limit: i64,
    lease_seconds: i64,
}

#[derive(Debug, PartialEq, Eq)]
enum ClaimParameterError {
    Limit,
    LeaseSeconds,
}

impl ClaimParameterError {
    fn message(&self) -> String {
        match self {
            Self::Limit => format!("limit must be between 1 and {MAX_CLAIM_LIMIT}"),
            Self::LeaseSeconds => "lease_seconds must be positive".into(),
        }
    }
}

impl ClaimEmbeddingsQuery {
    fn validate(self) -> Validation<ClaimParameters, ClaimParameterError> {
        let limit = self.limit.unwrap_or(32);
        let lease_seconds = self.lease_seconds.unwrap_or(300);
        let limit = Validation::from(
            (1..=MAX_CLAIM_LIMIT)
                .contains(&limit)
                .then_some(limit)
                .ok_or(ClaimParameterError::Limit),
        );
        let lease_seconds = Validation::from(
            (lease_seconds > 0)
                .then_some(lease_seconds)
                .ok_or(ClaimParameterError::LeaseSeconds),
        );
        limit.map2(lease_seconds, |limit, lease_seconds| ClaimParameters {
            limit,
            lease_seconds,
        })
    }
}

#[utoipa::path(
    post,
    path = "/internal/embeddings/claim",
    params(
        ("limit" = Option<i64>, Query, description = "Maximum number of items to claim (1-256, default 32)"),
        ("lease_seconds" = Option<i64>, Query, description = "Lease duration in seconds (default 300)")
    ),
    responses(
        (status = 200, description = "Items to embed and the model to use", body = ClaimEmbeddingsResponse),
        (status = 400, description = "Invalid parameters"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "embeddings"
)]
#[instrument(skip(state))]
pub async fn claim_embeddings(
    State(state): State<SearchState>,
    Query(query): Query<ClaimEmbeddingsQuery>,
) -> Result<Json<ClaimEmbeddingsResponse>, HandlerError> {
    let ClaimParameters {
        limit,
        lease_seconds,
    } = query.validate().into_result().map_err(|errors| {
        for error in &errors {
            debug!(error = %error.message(), "invalid embedding claim parameter");
        }
        // Keep the existing first-error HTTP contract.
        HandlerError::bad_request(errors.first().message())
    })?;

    let jobs = claim_items_needing_embedding(&state.pool, &state.model, limit, lease_seconds)
        .await
        .map_err(|e| HandlerError::from_db(e, "Failed to claim items for embedding"))?;
    Ok(Json(ClaimEmbeddingsResponse {
        model: state.model.clone(),
        jobs,
    }))
}

#[utoipa::path(
    put,
    path = "/internal/embeddings",
    request_body = StoreEmbeddingsRequest,
    responses(
        (status = 200, description = "Embeddings stored", body = StoreEmbeddingsResult),
        (status = 400, description = "Invalid embeddings"),
        (status = 409, description = "Embeddings were produced with a different model"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "embeddings"
)]
#[instrument(skip(state, request), fields(count = request.embeddings.len()))]
pub async fn store_embeddings(
    State(state): State<SearchState>,
    Json(request): Json<StoreEmbeddingsRequest>,
) -> Result<Json<StoreEmbeddingsResult>, HandlerError> {
    if request.model != state.model {
        return Err(HandlerError::conflict(format!(
            "embeddings were produced with model {} but the backend uses {}",
            request.model, state.model
        )));
    }
    if request
        .embeddings
        .iter()
        .any(|e| rss_centr_core::semantic::normalize(&e.embedding).is_none())
    {
        return Err(HandlerError::bad_request(
            "embeddings must be non-empty, finite and non-zero",
        ));
    }

    let stored = store_item_embeddings(&state.pool, &state.model, &request.embeddings)
        .await
        .map_err(|e| HandlerError::from_db(e, "Failed to store embeddings"))?;
    Ok(Json(StoreEmbeddingsResult { stored }))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::feed::feed_item::insert_feed_item;
    use crate::feed::feed_subscription::upsert_feed_by_url;
    use crate::search::{EmbedFuture, QueryEmbedder};
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{post, put};
    use http_body_util::BodyExt;
    use rss_centr_core::semantic::ItemEmbedding;
    use sqlx::PgPool;
    use tower::ServiceExt;

    struct NoEmbedder;

    impl QueryEmbedder for NoEmbedder {
        fn embed_query<'a>(&'a self, _query: &'a str) -> EmbedFuture<'a> {
            Box::pin(async { anyhow::bail!("not used") })
        }
    }

    fn app(pool: PgPool) -> Router {
        Router::new()
            .route("/embeddings/claim", post(claim_embeddings))
            .route("/embeddings", put(store_embeddings))
            .with_state(SearchState {
                pool,
                embedder: Arc::new(NoEmbedder),
                model: "test-model".to_string(),
                centroids: Arc::new(crate::search::EmbeddingCentroids::new("test-model")),
                min_similarity: 0.3,
                recency: rss_centr_core::semantic::RecencyBoost::NONE,
            })
    }

    async fn send(pool: &PgPool, request: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = app(pool.clone()).oneshot(request).await.unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null),
        )
    }

    fn claim(uri: &str) -> Request<Body> {
        Request::post(uri).body(Body::empty()).unwrap()
    }

    fn store(request: &StoreEmbeddingsRequest) -> Request<Body> {
        Request::put("/embeddings")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(request).unwrap()))
            .unwrap()
    }

    #[test]
    fn claim_parameter_validation_accumulates_errors_in_field_order() {
        let errors = ClaimEmbeddingsQuery {
            limit: Some(0),
            lease_seconds: Some(0),
        }
        .validate()
        .into_result()
        .unwrap_err();
        assert_eq!(
            errors.into_vec(),
            [
                ClaimParameterError::Limit,
                ClaimParameterError::LeaseSeconds
            ]
        );
    }

    #[test]
    fn claim_parameter_validation_preserves_defaults_and_boundaries() {
        for (limit, lease_seconds, expected) in [
            (
                None,
                None,
                ClaimParameters {
                    limit: 32,
                    lease_seconds: 300,
                },
            ),
            (
                Some(1),
                Some(1),
                ClaimParameters {
                    limit: 1,
                    lease_seconds: 1,
                },
            ),
            (
                Some(MAX_CLAIM_LIMIT),
                Some(i64::MAX),
                ClaimParameters {
                    limit: MAX_CLAIM_LIMIT,
                    lease_seconds: i64::MAX,
                },
            ),
        ] {
            assert_eq!(
                ClaimEmbeddingsQuery {
                    limit,
                    lease_seconds
                }
                .validate()
                .into_result(),
                Ok(expected)
            );
        }
    }

    quickcheck::quickcheck! {
        fn prop_claim_validation_accepts_exactly_valid_ranges(limit: i64, lease_seconds: i64) -> bool {
            let result = ClaimEmbeddingsQuery {
                limit: Some(limit),
                lease_seconds: Some(lease_seconds),
            }.validate().into_result();
            match result {
                Ok(parameters) => (1..=MAX_CLAIM_LIMIT).contains(&limit)
                    && lease_seconds > 0
                    && parameters.limit == limit
                    && parameters.lease_seconds == lease_seconds,
                Err(errors) => {
                    let expected: Vec<_> = [
                        (!(1..=MAX_CLAIM_LIMIT).contains(&limit)).then_some(ClaimParameterError::Limit),
                        (lease_seconds <= 0).then_some(ClaimParameterError::LeaseSeconds),
                    ].into_iter().flatten().collect();
                    errors.into_vec() == expected
                }
            }
        }
    }

    #[tokio::test]
    async fn invalid_claims_preserve_first_error_without_accessing_database() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@127.0.0.1:1/unused")
            .unwrap();
        pool.close().await;
        for (query, message) in [
            (
                ClaimEmbeddingsQuery {
                    limit: Some(0),
                    lease_seconds: Some(0),
                },
                "limit must be between 1 and 256",
            ),
            (
                ClaimEmbeddingsQuery {
                    limit: Some(257),
                    lease_seconds: None,
                },
                "limit must be between 1 and 256",
            ),
            (
                ClaimEmbeddingsQuery {
                    limit: None,
                    lease_seconds: Some(-1),
                },
                "lease_seconds must be positive",
            ),
        ] {
            let state = SearchState {
                pool: pool.clone(),
                embedder: Arc::new(NoEmbedder),
                model: "test-model".into(),
                centroids: Arc::new(crate::search::EmbeddingCentroids::new("test-model")),
                min_similarity: 0.3,
                recency: rss_centr_core::semantic::RecencyBoost::NONE,
            };
            let error = claim_embeddings(State(state), Query(query))
                .await
                .unwrap_err();
            assert_eq!(error.status, StatusCode::BAD_REQUEST);
            assert_eq!(error.message, message);
        }
    }

    #[sqlx::test]
    async fn claim_then_store_round_trip(pool: PgPool) {
        sqlx::query!("DELETE FROM feed_items")
            .execute(&pool)
            .await
            .unwrap();
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let item = insert_feed_item(&pool, feed.id, "a", "Hello", "https://x/a")
            .await
            .unwrap();

        let (status, body) = send(&pool, claim("/embeddings/claim?limit=10")).await;
        assert_eq!(status, StatusCode::OK);
        let claimed: ClaimEmbeddingsResponse = serde_json::from_value(body).unwrap();
        assert_eq!(claimed.model, "test-model");
        assert_eq!(claimed.jobs.len(), 1);
        assert_eq!(claimed.jobs[0].feed_item_id, item.id);

        let request = StoreEmbeddingsRequest {
            model: claimed.model,
            embeddings: vec![ItemEmbedding {
                feed_item_id: item.id,
                content_hash: claimed.jobs[0].content_hash.clone(),
                embedding: vec![1.0, 0.0],
            }],
        };
        let (status, body) = send(&pool, store(&request)).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["stored"], 1);

        let (_, body) = send(&pool, claim("/embeddings/claim?limit=10")).await;
        assert_eq!(body["jobs"].as_array().unwrap().len(), 0);
    }

    #[sqlx::test]
    async fn claim_validates_parameters(pool: PgPool) {
        for uri in [
            "/embeddings/claim?limit=0",
            "/embeddings/claim?limit=10000",
            "/embeddings/claim?lease_seconds=0",
        ] {
            let (status, _) = send(&pool, claim(uri)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{uri}");
        }
    }

    #[sqlx::test]
    async fn store_rejects_other_models_and_bad_vectors(pool: PgPool) {
        let mut request = StoreEmbeddingsRequest {
            model: "other-model".to_string(),
            embeddings: vec![],
        };
        let (status, _) = send(&pool, store(&request)).await;
        assert_eq!(status, StatusCode::CONFLICT);

        request.model = "test-model".to_string();
        request.embeddings = vec![ItemEmbedding {
            feed_item_id: 1,
            content_hash: "h".to_string(),
            embedding: vec![0.0, 0.0],
        }];
        let (status, _) = send(&pool, store(&request)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
