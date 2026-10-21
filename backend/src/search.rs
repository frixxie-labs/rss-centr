use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::Duration,
};

use anyhow::Result;
use axum::extract::FromRef;
use metrics::histogram;
use rss_centr_core::{
    ollama,
    semantic::{RecencyBoost, query_input},
};
use sqlx::PgPool;
use tokio::time::Instant;
use tracing::info;

use crate::feed::item_embedding::EmbeddingIndex;

pub const DEFAULT_EMBED_MODEL: &str = "embeddinggemma-2";
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
    pub recency: RecencyBoost,
}

impl SearchState {
    pub fn from_env(pool: PgPool) -> Self {
        let model =
            std::env::var("OLLAMA_EMBED_MODEL").unwrap_or_else(|_| DEFAULT_EMBED_MODEL.to_string());
        let min_similarity = std::env::var("SEMANTIC_MIN_SIMILARITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(DEFAULT_MIN_SIMILARITY);
        let env_f32 = |name: &str| std::env::var(name).ok().and_then(|v| v.parse::<f32>().ok());
        let recency = RecencyBoost {
            weight: env_f32("SEMANTIC_RECENCY_WEIGHT").unwrap_or(RecencyBoost::DEFAULT_WEIGHT),
            half_life_hours: env_f32("SEMANTIC_RECENCY_HALF_LIFE_HOURS")
                .unwrap_or(RecencyBoost::DEFAULT_HALF_LIFE_HOURS),
        };
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
            recency,
        }
    }
}

impl FromRef<SearchState> for PgPool {
    fn from_ref(state: &SearchState) -> Self {
        state.pool.clone()
    }
}

/// A stage of a ranked (semantic or hybrid) search, see [`SearchTimings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStage {
    /// Embedding the query (instant for cached queries).
    Embed,
    /// Pulling embeddings stored since the last search into the index.
    Refresh,
    /// Scoring every indexed item against the query.
    Scan,
    /// The keyword query of hybrid search.
    Keyword,
    /// Ranking by relevance, keywords and recency.
    Rank,
    /// Reading the returned items.
    Load,
}

impl SearchStage {
    pub const ALL: [Self; 6] = [
        Self::Embed,
        Self::Refresh,
        Self::Scan,
        Self::Keyword,
        Self::Rank,
        Self::Load,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Embed => "embed",
            Self::Refresh => "refresh",
            Self::Scan => "scan",
            Self::Keyword => "keyword",
            Self::Rank => "rank",
            Self::Load => "load",
        }
    }
}

/// Counts logged with a search's timings.
#[derive(Debug, Default, Clone, Copy)]
pub struct SearchCounts {
    /// Embeddings in the in-memory index.
    pub indexed: usize,
    /// Items scored by similarity, after the feed filter.
    pub scored: usize,
    /// Items matching the keywords (hybrid search).
    pub keyword_matches: usize,
    /// Items returned.
    pub results: usize,
}

/// Times the stages of one ranked search. Each stage is recorded in the
/// `search_stage` histogram (labels `mode`, `stage`), and [`Self::finish`]
/// logs all of them with the total in one line.
pub struct SearchTimings {
    mode: &'static str,
    started: Instant,
    durations: [Option<Duration>; SearchStage::ALL.len()],
}

impl SearchTimings {
    pub fn new(mode: &'static str) -> Self {
        Self {
            mode,
            started: Instant::now(),
            durations: [None; SearchStage::ALL.len()],
        }
    }

    /// Await `future`, adding the time it takes to `stage`.
    pub async fn time<F: Future>(&mut self, stage: SearchStage, future: F) -> F::Output {
        let started = Instant::now();
        let output = future.await;
        let elapsed = started.elapsed();
        histogram!("search_stage", "mode" => self.mode, "stage" => stage.name()).record(elapsed);
        let total = &mut self.durations[stage as usize];
        *total = Some(total.unwrap_or_default() + elapsed);
        output
    }

    pub fn duration(&self, stage: SearchStage) -> Option<Duration> {
        self.durations[stage as usize]
    }

    /// Time since the search started.
    pub fn total(&self) -> Duration {
        self.started.elapsed()
    }

    /// Record the total and log every stage. Stages that did not run are
    /// left out of the log line.
    pub fn finish(self, counts: SearchCounts) {
        let total = self.total();
        histogram!("search_stage", "mode" => self.mode, "stage" => "total").record(total);
        let ms = |stage| self.duration(stage).map(millis);
        info!(
            mode = self.mode,
            total_ms = millis(total),
            embed_ms = ms(SearchStage::Embed),
            refresh_ms = ms(SearchStage::Refresh),
            scan_ms = ms(SearchStage::Scan),
            keyword_ms = ms(SearchStage::Keyword),
            rank_ms = ms(SearchStage::Rank),
            load_ms = ms(SearchStage::Load),
            indexed = counts.indexed,
            scored = counts.scored,
            keyword_matches = counts.keyword_matches,
            results = counts.results,
            "search finished"
        );
    }
}

/// Milliseconds with two decimals, readable in logs.
fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 100_000.0).round() / 100.0
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

    #[tokio::test]
    async fn timings_return_the_output_and_record_the_stage() {
        let mut timings = SearchTimings::new("hybrid");
        let output = timings
            .time(SearchStage::Scan, async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                42
            })
            .await;
        assert_eq!(output, 42);
        assert!(timings.duration(SearchStage::Scan).unwrap() >= Duration::from_millis(5));
        assert_eq!(timings.duration(SearchStage::Keyword), None, "not run");
    }

    #[tokio::test]
    async fn timings_add_up_repeated_stages() {
        let mut timings = SearchTimings::new("semantic");
        for _ in 0..2 {
            timings
                .time(
                    SearchStage::Load,
                    tokio::time::sleep(Duration::from_millis(3)),
                )
                .await;
        }
        assert!(timings.duration(SearchStage::Load).unwrap() >= Duration::from_millis(6));
    }

    #[tokio::test]
    async fn total_covers_every_stage() {
        let mut timings = SearchTimings::new("semantic");
        timings
            .time(
                SearchStage::Embed,
                tokio::time::sleep(Duration::from_millis(2)),
            )
            .await;
        timings
            .time(
                SearchStage::Rank,
                tokio::time::sleep(Duration::from_millis(2)),
            )
            .await;
        let stages = timings.duration(SearchStage::Embed).unwrap()
            + timings.duration(SearchStage::Rank).unwrap();
        assert!(timings.total() >= stages);
    }

    #[test]
    fn stage_names_are_distinct_metric_labels() {
        let names: std::collections::HashSet<&str> =
            SearchStage::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names.len(), SearchStage::ALL.len());
    }
}
