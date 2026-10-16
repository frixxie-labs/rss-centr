use std::{future::Future, pin::Pin, sync::Arc};

use anyhow::Result;
use axum::extract::FromRef;
use rss_centr_core::{ollama, semantic::query_input};
use sqlx::PgPool;

pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma";
pub const DEFAULT_MIN_SIMILARITY: f32 = 0.2;

pub type EmbedFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<f32>>> + Send + 'a>>;

/// Turns a search query into a normalized embedding vector.
pub trait QueryEmbedder: Send + Sync {
    fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a>;
}

pub struct OllamaQueryEmbedder {
    http: reqwest::Client,
    ollama_url: String,
    model: String,
}

impl QueryEmbedder for OllamaQueryEmbedder {
    fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let inputs = [query_input(query)];
            let mut vectors =
                ollama::embed(&self.http, &self.ollama_url, &self.model, &inputs).await?;
            vectors
                .pop()
                .ok_or_else(|| anyhow::anyhow!("Ollama returned no query embedding"))
        })
    }
}

#[derive(Clone)]
pub struct SearchState {
    pub pool: PgPool,
    pub embedder: Arc<dyn QueryEmbedder>,
    pub model: String,
    pub min_similarity: f32,
}

impl SearchState {
    pub fn from_env(pool: PgPool) -> Self {
        let model =
            std::env::var("OLLAMA_EMBED_MODEL").unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_string());
        let min_similarity = std::env::var("SEMANTIC_MIN_SIMILARITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MIN_SIMILARITY);
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self {
            pool,
            embedder: Arc::new(OllamaQueryEmbedder {
                http,
                ollama_url: std::env::var("OLLAMA_URL")
                    .unwrap_or_else(|_| "http://desktop:11434".to_string()),
                model: model.clone(),
            }),
            model,
            min_similarity,
        }
    }
}

impl FromRef<SearchState> for PgPool {
    fn from_ref(state: &SearchState) -> Self {
        state.pool.clone()
    }
}
