use axum::{Json, extract::Query, extract::State};
use rss_centr_core::semantic::{
    ClaimEmbeddingsResponse, StoreEmbeddingsRequest, StoreEmbeddingsResult,
};
use serde::Deserialize;
use tracing::instrument;

use crate::search::SearchState;
use crate::search::embedding_store::{claim_items_needing_embedding, store_item_embeddings};

use super::error::HandlerError;

const MAX_CLAIM_LIMIT: i64 = 256;

#[derive(Debug, Deserialize)]
pub struct ClaimEmbeddingsQuery {
    pub limit: Option<i64>,
    pub lease_seconds: Option<i64>,
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
    let limit = query.limit.unwrap_or(32);
    let lease_seconds = query.lease_seconds.unwrap_or(300);
    if !(1..=MAX_CLAIM_LIMIT).contains(&limit) {
        return Err(HandlerError::bad_request(format!(
            "limit must be between 1 and {MAX_CLAIM_LIMIT}"
        )));
    }
    if lease_seconds <= 0 {
        return Err(HandlerError::bad_request("lease_seconds must be positive"));
    }

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
                embeddings: Arc::new(crate::search::EmbeddingIndex::new("test-model")),
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
