use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use anyhow::Result;
use axum::extract::FromRef;
use rss_centr_core::{ollama, semantic::query_input};
use sqlx::PgPool;

use crate::feed::item_embedding::EmbeddingIndex;

pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma";
pub const DEFAULT_MIN_SIMILARITY: f32 = 0.2;
/// Keep the embedding model loaded between searches; loading it takes ~2 s.
pub const DEFAULT_EMBED_KEEP_ALIVE: &str = "1h";

pub type EmbedFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<f32>>> + Send + 'a>>;

/// Turns a search query into a normalized embedding vector.
pub trait QueryEmbedder: Send + Sync {
    fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a>;
}

pub struct OllamaQueryEmbedder {
    http: reqwest::Client,
    ollama_url: String,
    model: String,
    keep_alive: String,
}

impl QueryEmbedder for OllamaQueryEmbedder {
    fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let inputs = [query_input(query)];
            let mut vectors = ollama::embed(
                &self.http,
                &self.ollama_url,
                &self.model,
                &inputs,
                Some(&self.keep_alive),
            )
            .await?;
            vectors
                .pop()
                .ok_or_else(|| anyhow::anyhow!("Ollama returned no query embedding"))
        })
    }
}

/// Number of recent query embeddings kept in memory.
pub const QUERY_CACHE_CAPACITY: usize = 256;

/// Caches query embeddings in memory, evicting the least recently used entry.
pub struct CachingEmbedder {
    inner: Arc<dyn QueryEmbedder>,
    capacity: usize,
    entries: Mutex<QueryCache>,
}

#[derive(Default)]
struct QueryCache {
    vectors: HashMap<String, (Vec<f32>, u64)>,
    clock: u64,
}

impl CachingEmbedder {
    pub fn new(inner: Arc<dyn QueryEmbedder>, capacity: usize) -> Self {
        Self {
            inner,
            capacity: capacity.max(1),
            entries: Mutex::new(QueryCache::default()),
        }
    }

    fn get(&self, key: &str) -> Option<Vec<f32>> {
        let mut cache = self.entries.lock().ok()?;
        cache.clock += 1;
        let clock = cache.clock;
        cache.vectors.get_mut(key).map(|(vector, used)| {
            *used = clock;
            vector.clone()
        })
    }

    fn insert(&self, key: String, vector: Vec<f32>) {
        let Ok(mut cache) = self.entries.lock() else {
            return;
        };
        cache.clock += 1;
        let clock = cache.clock;
        cache.vectors.insert(key, (vector, clock));
        while cache.vectors.len() > self.capacity {
            let oldest = cache
                .vectors
                .iter()
                .min_by_key(|(_, (_, used))| *used)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => cache.vectors.remove(&key),
                None => break,
            };
        }
    }
}

impl QueryEmbedder for CachingEmbedder {
    fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a> {
        Box::pin(async move {
            let key = query.trim().to_string();
            if let Some(vector) = self.get(&key) {
                return Ok(vector);
            }
            let vector = self.inner.embed_query(&key).await?;
            self.insert(key, vector.clone());
            Ok(vector)
        })
    }
}

#[derive(Clone)]
pub struct SearchState {
    pub pool: PgPool,
    pub embedder: Arc<dyn QueryEmbedder>,
    pub model: String,
    pub embeddings: Arc<EmbeddingIndex>,
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
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_default();
        Self {
            pool,
            embedder: Arc::new(CachingEmbedder::new(
                Arc::new(OllamaQueryEmbedder {
                    http,
                    ollama_url: std::env::var("OLLAMA_URL")
                        .unwrap_or_else(|_| "http://desktop:11434".to_string()),
                    model: model.clone(),
                    keep_alive: std::env::var("OLLAMA_EMBED_KEEP_ALIVE")
                        .unwrap_or_else(|_| DEFAULT_EMBED_KEEP_ALIVE.to_string()),
                }),
                QUERY_CACHE_CAPACITY,
            )),
            embeddings: Arc::new(EmbeddingIndex::new(model.clone())),
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Embeds a query as `[len]`, counting calls; queries starting with
    /// "fail" return an error.
    #[derive(Default)]
    struct CountingEmbedder(AtomicUsize);

    impl QueryEmbedder for CountingEmbedder {
        fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move {
                if query.starts_with("fail") {
                    anyhow::bail!("unavailable");
                }
                Ok(vec![query.len() as f32])
            })
        }
    }

    fn cached(capacity: usize) -> (Arc<CountingEmbedder>, CachingEmbedder) {
        let inner = Arc::new(CountingEmbedder::default());
        (inner.clone(), CachingEmbedder::new(inner, capacity))
    }

    #[tokio::test]
    async fn repeated_queries_are_embedded_once() {
        let (inner, cache) = cached(8);
        assert_eq!(cache.embed_query("rust").await.unwrap(), vec![4.0]);
        assert_eq!(cache.embed_query("rust").await.unwrap(), vec![4.0]);
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);

        cache.embed_query("electric cars").await.unwrap();
        assert_eq!(inner.0.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn surrounding_whitespace_shares_a_cache_entry() {
        let (inner, cache) = cached(8);
        cache.embed_query("rust").await.unwrap();
        cache.embed_query("  rust ").await.unwrap();
        assert_eq!(inner.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failures_are_not_cached() {
        let (inner, cache) = cached(8);
        assert!(cache.embed_query("fail").await.is_err());
        assert!(cache.embed_query("fail").await.is_err());
        assert_eq!(inner.0.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn least_recently_used_entries_are_evicted() {
        let (inner, cache) = cached(2);
        cache.embed_query("a").await.unwrap();
        cache.embed_query("b").await.unwrap();
        cache.embed_query("a").await.unwrap(); // "a" is now most recent
        cache.embed_query("c").await.unwrap(); // evicts "b"
        assert_eq!(inner.0.load(Ordering::SeqCst), 3);

        cache.embed_query("a").await.unwrap();
        assert_eq!(inner.0.load(Ordering::SeqCst), 3, "a is still cached");
        cache.embed_query("b").await.unwrap();
        assert_eq!(inner.0.load(Ordering::SeqCst), 4, "b was evicted");
    }
}
