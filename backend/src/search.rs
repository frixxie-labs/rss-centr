mod centroids;
mod config;
mod embedder;
pub mod embedding_store;
mod items;
mod timings;

pub(crate) use centroids::FeedCentroid;
pub use centroids::{Centroids, EmbeddingCentroids};
pub(crate) use config::SearchConfig;
pub use config::{DEFAULT_EMBED_KEEP_ALIVE, DEFAULT_EMBED_MODEL, DEFAULT_MIN_SIMILARITY};
use embedder::OllamaQueryEmbedder;
pub use embedder::{CachingEmbedder, EmbedFuture, QUERY_CACHE_CAPACITY, QueryEmbedder};
pub(crate) use items::{LatestItemsQuery, SearchError, SearchMode, search_items};
pub use timings::{SearchCounts, SearchStage, SearchTimings};

use std::sync::Arc;

use axum::extract::FromRef;
use rss_centr_core::semantic::RecencyBoost;
use sqlx::PgPool;

#[derive(Clone)]
pub struct SearchState {
    pub pool: PgPool,
    pub embedder: Arc<dyn QueryEmbedder>,
    pub model: String,
    pub centroids: Arc<EmbeddingCentroids>,
    pub min_similarity: f32,
    pub recency: RecencyBoost,
}

impl SearchState {
    pub(crate) fn new(
        pool: PgPool,
        http: reqwest::Client,
        ollama_url: String,
        config: SearchConfig,
    ) -> Self {
        let SearchConfig {
            model,
            keep_alive,
            min_similarity,
            recency,
        } = config;
        Self {
            pool,
            embedder: Arc::new(CachingEmbedder::new(
                Arc::new(OllamaQueryEmbedder::new(
                    http,
                    ollama_url,
                    model.clone(),
                    keep_alive,
                )),
                QUERY_CACHE_CAPACITY,
            )),
            centroids: Arc::new(EmbeddingCentroids::new(model.clone())),
            model,
            min_similarity,
            recency,
        }
    }
}

impl FromRef<SearchState> for PgPool {
    fn from_ref(state: &SearchState) -> Self {
        state.pool.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn state_uses_the_injected_client_endpoint_model_and_ranking_settings() {
        use axum::{Json, Router, http::HeaderMap, routing::post};

        let app = Router::new().route(
            "/configured/api/embed",
            post(
                |headers: HeaderMap, Json(request): Json<serde_json::Value>| async move {
                    assert_eq!(headers["x-configured-client"], "search");
                    assert_eq!(request["model"], "configured-embed-model");
                    assert_eq!(request["keep_alive"], "30m");
                    assert_eq!(
                        request["input"],
                        serde_json::json!(["task: search result | query: rust"])
                    );
                    Json(serde_json::json!({"embeddings": [[3.0, 4.0]]}))
                },
            ),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("x-configured-client", "search".parse().unwrap());
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .unwrap();
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://postgres:postgres@localhost/rss_centr")
            .unwrap();
        let recency = RecencyBoost {
            weight: 0.75,
            half_life_hours: 12.0,
        };

        let state = SearchState::new(
            pool,
            http,
            format!("http://{address}/configured/"),
            SearchConfig {
                model: "configured-embed-model".into(),
                keep_alive: "30m".into(),
                min_similarity: 0.4,
                recency,
            },
        );

        assert_eq!(state.model, "configured-embed-model");
        assert_eq!(state.centroids.model(), state.model);
        assert_eq!(state.min_similarity, 0.4);
        assert_eq!(state.recency, recency);
        assert_eq!(
            state.embedder.embed_query("rust").await.unwrap(),
            vec![0.6, 0.8]
        );
        server.abort();
    }
}
