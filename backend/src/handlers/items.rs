use axum::{Json, extract::Path, extract::Query, extract::State};
use sqlx::PgPool;
use tracing::{instrument, warn};

use crate::feed::feed_item::{FeedItem, FeedItemDetail, FeedItemWithDetail};
use crate::feed::feed_item::{read_feed_item, read_feed_item_detail, read_feed_items_by_feed};
use crate::search::{LatestItemsQuery, SearchMode, SearchState, search_items};

use super::error::HandlerError;

#[utoipa::path(
    get,
    path = "/api/feeds/{feed_id}/items",
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "List of feed items", body = [FeedItem]),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_items_by_feed(
    State(pool): State<PgPool>,
    Path(feed_id): Path<i64>,
) -> Result<Json<Vec<FeedItem>>, HandlerError> {
    let items = read_feed_items_by_feed(&pool, feed_id).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(items))
}

#[utoipa::path(
    get,
    path = "/api/items/latest",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of items to return"),
        ("offset" = Option<u32>, Query, description = "Number of items to skip, for paging"),
        ("feed_id" = Option<i64>, Query, description = "Only return items from this feed"),
        ("q" = Option<String>, Query, description = "Search items by title, URL, summary, content, author, or feed"),
        ("mode" = Option<SearchMode>, Query, description = "Search mode: keyword (default, newest first), semantic (by meaning) or hybrid (both, ranked by relevance)")
    ),
    responses(
        (status = 200, description = "List of latest feed items", body = [FeedItemWithDetail]),
        (status = 400, description = "Invalid query parameters"),
        (status = 500, description = "Internal server error"),
        (status = 503, description = "Semantic search is unavailable"),
    ),
    tag = "items"
)]
#[instrument(skip(state))]
pub async fn fetch_latest_items(
    State(state): State<SearchState>,
    Query(query): Query<LatestItemsQuery>,
) -> Result<Json<Vec<FeedItemWithDetail>>, HandlerError> {
    Ok(Json(
        search_items(&state, &query, chrono::Utc::now()).await?,
    ))
}

#[utoipa::path(
    get,
    path = "/api/items/{item_id}",
    params(
        ("item_id" = i64, Path, description = "Feed item ID")
    ),
    responses(
        (status = 200, description = "Feed item", body = FeedItem),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_item_by_id(
    State(pool): State<PgPool>,
    Path(item_id): Path<i64>,
) -> Result<Json<FeedItem>, HandlerError> {
    let item = read_feed_item(&pool, item_id).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(item))
}

#[utoipa::path(
    get,
    path = "/api/items/{item_id}/detail",
    params(
        ("item_id" = i64, Path, description = "Feed item ID")
    ),
    responses(
        (status = 200, description = "Feed item detail", body = FeedItemDetail),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_item_detail(
    State(pool): State<PgPool>,
    Path(item_id): Path<i64>,
) -> Result<Json<FeedItemDetail>, HandlerError> {
    let detail = read_feed_item_detail(&pool, item_id).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(detail))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use super::*;
    use crate::feed::feed_item::{insert_feed_item, insert_feed_item_detail};
    use crate::feed::feed_subscription::upsert_feed_by_url;
    use crate::feed::item_embedding::store_item_embeddings;
    use crate::search::{EmbedFuture, QueryEmbedder};
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::http::StatusCode;
    use axum::routing::get;
    use chrono::{Duration, Utc};
    use http_body_util::BodyExt;
    use rss_centr_core::semantic::ItemEmbedding;
    use tower::ServiceExt;

    const MODEL: &str = "test-model";

    /// Maps known queries to fixed vectors; any other query fails like an
    /// unreachable Ollama would.
    struct FakeEmbedder(HashMap<&'static str, Vec<f32>>);

    impl QueryEmbedder for FakeEmbedder {
        fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a> {
            let result = self
                .0
                .get(query)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("embedding service unavailable"));
            Box::pin(async move { result })
        }
    }

    fn app(pool: PgPool) -> Router {
        let embedder = FakeEmbedder(HashMap::from([
            ("automobile", vec![1.0, 0.0]),
            ("car", vec![1.0, 0.0]),
        ]));
        let state = SearchState {
            pool,
            embedder: Arc::new(embedder),
            model: MODEL.to_string(),
            embeddings: Arc::new(crate::search::EmbeddingIndex::new(MODEL)),
            min_similarity: 0.3,
            recency: rss_centr_core::semantic::RecencyBoost::NONE,
        };
        Router::new()
            .route("/items/latest", get(fetch_latest_items))
            .with_state(state)
    }

    struct Fixture {
        feed_a: i64,
        /// "Electric vehicles" - semantically about cars, no keyword "car".
        ev: i64,
        /// "Car repair tips" - keyword match for "car", semantically unrelated.
        repair: i64,
        /// "Sourdough baking" - unrelated in every way.
        baking: i64,
        /// Another car-like item in a different feed.
        other_feed_ev: i64,
    }

    async fn fixture(pool: &PgPool) -> Fixture {
        sqlx::query!("DELETE FROM feed_items")
            .execute(pool)
            .await
            .unwrap();
        let feed_a = upsert_feed_by_url(pool, "https://a.example.com/feed.xml")
            .await
            .unwrap()
            .id;
        let feed_b = upsert_feed_by_url(pool, "https://b.example.com/feed.xml")
            .await
            .unwrap()
            .id;
        let now = Utc::now();
        let add = async |feed_id: i64, key: &str, title: &str, age_hours: i64, v: [f32; 2]| {
            let item = insert_feed_item(pool, feed_id, key, title, &format!("https://x/{key}"))
                .await
                .unwrap();
            insert_feed_item_detail(pool, item.id, "", "", "", now - Duration::hours(age_hours))
                .await
                .unwrap();
            store_item_embeddings(
                pool,
                MODEL,
                &[ItemEmbedding {
                    feed_item_id: item.id,
                    content_hash: "h".to_string(),
                    embedding: v.to_vec(),
                }],
            )
            .await
            .unwrap();
            item.id
        };
        let ev = add(
            feed_a,
            "ev",
            "Electric vehicles get cheaper",
            3,
            [0.95, 0.05],
        )
        .await;
        let repair = add(feed_a, "repair", "Car repair tips", 1, [0.0, 1.0]).await;
        let baking = add(feed_a, "baking", "Sourdough baking", 0, [0.0, 1.0]).await;
        let other_feed_ev = add(feed_b, "ev2", "Hybrid sedans reviewed", 2, [0.9, 0.1]).await;
        Fixture {
            feed_a,
            ev,
            repair,
            baking,
            other_feed_ev,
        }
    }

    async fn get_items(pool: &PgPool, uri: &str) -> (StatusCode, Vec<i64>) {
        let response = app(pool.clone())
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        if status != StatusCode::OK {
            return (status, Vec::new());
        }
        let items: Vec<FeedItemWithDetail> = serde_json::from_slice(&body).unwrap();
        (status, items.into_iter().map(|i| i.id).collect())
    }

    #[sqlx::test]
    async fn keyword_mode_is_the_default_and_unchanged(pool: PgPool) {
        let f = fixture(&pool).await;
        for uri in ["/items/latest?q=car", "/items/latest?q=car&mode=keyword"] {
            let (status, ids) = get_items(&pool, uri).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(ids, vec![f.repair], "{uri}");
        }
    }

    #[sqlx::test]
    async fn empty_query_returns_latest_items_in_every_mode(pool: PgPool) {
        let f = fixture(&pool).await;
        let expected = vec![f.baking, f.repair, f.other_feed_ev, f.ev];
        for mode in ["keyword", "semantic", "hybrid"] {
            let (status, ids) = get_items(&pool, &format!("/items/latest?q=+&mode={mode}")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(ids, expected, "{mode}");
        }
    }

    #[sqlx::test]
    async fn semantic_mode_finds_items_by_meaning_without_keyword_overlap(pool: PgPool) {
        let f = fixture(&pool).await;
        let (status, ids) = get_items(&pool, "/items/latest?q=automobile&mode=semantic").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids, vec![f.ev, f.other_feed_ev]);
    }

    #[sqlx::test]
    async fn semantic_mode_respects_feed_filter_and_limit(pool: PgPool) {
        let f = fixture(&pool).await;
        let (_, ids) = get_items(
            &pool,
            &format!(
                "/items/latest?q=automobile&mode=semantic&feed_id={}",
                f.feed_a
            ),
        )
        .await;
        assert_eq!(ids, vec![f.ev]);

        let (_, ids) = get_items(&pool, "/items/latest?q=automobile&mode=semantic&limit=1").await;
        assert_eq!(ids, vec![f.ev]);
    }

    #[sqlx::test]
    async fn hybrid_mode_combines_keyword_and_semantic_matches(pool: PgPool) {
        let f = fixture(&pool).await;
        let (status, ids) = get_items(&pool, "/items/latest?q=car&mode=hybrid").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(
            ids,
            vec![f.ev, f.other_feed_ev, f.repair],
            "meaning matches outrank the keyword-only match, which is still kept"
        );
    }

    #[sqlx::test]
    async fn hybrid_mode_falls_back_to_keyword_when_embedding_fails(pool: PgPool) {
        let f = fixture(&pool).await;
        let (status, ids) = get_items(&pool, "/items/latest?q=repair&mode=hybrid").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ids, vec![f.repair]);
    }

    #[sqlx::test]
    async fn semantic_mode_reports_unavailable_when_embedding_fails(pool: PgPool) {
        fixture(&pool).await;
        let (status, _) = get_items(&pool, "/items/latest?q=repair&mode=semantic").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[sqlx::test]
    async fn ranked_search_pages_match_slices_of_the_full_ranking(pool: PgPool) {
        fixture(&pool).await;
        for (query, mode) in [("automobile", "semantic"), ("car", "hybrid")] {
            let base = format!("/items/latest?q={query}&mode={mode}");
            let (status, all) = get_items(&pool, &base).await;
            assert_eq!(status, StatusCode::OK);
            for offset in 0..=all.len() + 1 {
                let (status, page) =
                    get_items(&pool, &format!("{base}&limit=1&offset={offset}")).await;
                assert_eq!(status, StatusCode::OK);
                assert_eq!(
                    page,
                    all.iter().copied().skip(offset).take(1).collect::<Vec<_>>()
                );
            }
            let (status, page) = get_items(&pool, &format!("{base}&limit=0&offset=1")).await;
            assert_eq!(status, StatusCode::OK);
            assert!(page.is_empty());
        }
    }

    #[sqlx::test]
    async fn hybrid_fallback_preserves_keyword_order_and_pagination(pool: PgPool) {
        let f = fixture(&pool).await;
        let mut older = Vec::new();
        for age in [2, 3] {
            let key = format!("repair-{age}");
            let item = insert_feed_item(
                &pool,
                f.feed_a,
                &key,
                "Repair advice",
                &format!("https://a.example.com/{key}"),
            )
            .await
            .unwrap();
            insert_feed_item_detail(
                &pool,
                item.id,
                "",
                "",
                "",
                Utc::now() - Duration::hours(age),
            )
            .await
            .unwrap();
            older.push(item.id);
        }
        let (status, all) = get_items(&pool, "/items/latest?q=repair&mode=hybrid").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(all, vec![f.repair, older[0], older[1]]);

        let (status, page) =
            get_items(&pool, "/items/latest?q=repair&mode=hybrid&limit=1&offset=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page, vec![older[0]]);
    }

    #[sqlx::test]
    async fn query_whitespace_is_trimmed_before_keyword_search_and_embedding(pool: PgPool) {
        let f = fixture(&pool).await;
        for (query, mode, expected) in [
            ("car", "keyword", vec![f.repair]),
            ("automobile", "semantic", vec![f.ev, f.other_feed_ev]),
            ("car", "hybrid", vec![f.ev, f.other_feed_ev, f.repair]),
        ] {
            let (status, ids) =
                get_items(&pool, &format!("/items/latest?q=++{query}++&mode={mode}")).await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(ids, expected);
        }
    }

    #[sqlx::test]
    async fn unknown_mode_is_rejected(pool: PgPool) {
        let (status, _) = get_items(&pool, "/items/latest?q=car&mode=fuzzy").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
}
