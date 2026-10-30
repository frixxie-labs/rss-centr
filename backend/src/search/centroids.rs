use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Result;
use rss_centr_core::semantic::dot;
use sqlx::PgPool;
use tokio::sync::Mutex;
use tracing::{info, warn};

use super::embedding_store::read_feed_centroids;

const RELOAD_INTERVAL: Duration = Duration::from_secs(10 * 60);
const LOAD_RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// The sum and number of one feed's embeddings of a single dimension.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct FeedCentroid {
    pub(crate) feed_id: i64,
    pub(crate) sum: Vec<f32>,
    pub(crate) count: i64,
}

/// Per-feed embedding sums, enough to give any query's mean similarity
/// without reading every embedding.
#[derive(Debug, Default)]
pub struct Centroids {
    feeds: Vec<FeedCentroid>,
}

impl Centroids {
    pub(crate) fn new(feeds: Vec<FeedCentroid>) -> Self {
        Self { feeds }
    }

    /// The average similarity of `query` to the embeddings of the same
    /// dimension, optionally of one feed. Embeddings are unit vectors and the
    /// dot product is linear, so this is the query's dot product with their
    /// sum divided by their count. `None` if there are no such embeddings.
    pub fn mean_similarity(&self, query: &[f32], feed_id: Option<i64>) -> Option<f32> {
        let (sum, count) = self
            .feeds
            .iter()
            .filter(|feed| {
                feed.sum.len() == query.len() && feed_id.is_none_or(|id| id == feed.feed_id)
            })
            .fold((0.0f64, 0i64), |(sum, count), feed| {
                (sum + f64::from(dot(query, &feed.sum)), count + feed.count)
            });
        (count > 0).then(|| (sum / count as f64) as f32)
    }

    /// Number of embeddings summed.
    pub fn count(&self) -> usize {
        self.feeds.iter().map(|feed| feed.count as usize).sum()
    }
}

/// One model's [`Centroids`], loaded on first use and reloaded periodically.
/// Embeddings stored in between shift the mean only slightly.
pub struct EmbeddingCentroids {
    model: String,
    current: RwLock<Option<Arc<Centroids>>>,
    loading: Mutex<()>,
}

impl EmbeddingCentroids {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            current: RwLock::new(None),
            loading: Mutex::new(()),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    fn current(&self) -> Option<Arc<Centroids>> {
        self.current
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The loaded centroids, loading them first if necessary. Concurrent
    /// callers wait for a single load.
    pub async fn get(&self, pool: &PgPool) -> Result<Arc<Centroids>> {
        if let Some(centroids) = self.current() {
            return Ok(centroids);
        }
        let _guard = self.loading.lock().await;
        match self.current() {
            Some(centroids) => Ok(centroids),
            None => self.load(pool).await,
        }
    }

    async fn load(&self, pool: &PgPool) -> Result<Arc<Centroids>> {
        let centroids = Arc::new(Centroids::new(
            read_feed_centroids(pool, &self.model).await?,
        ));
        *self
            .current
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(centroids.clone());
        Ok(centroids)
    }

    /// Reload periodically, retrying failed loads sooner.
    pub async fn run_loader(&self, pool: &PgPool) {
        loop {
            let started = Instant::now();
            let loaded = {
                let _guard = self.loading.lock().await;
                self.load(pool).await
            };
            match loaded {
                Ok(centroids) => {
                    info!(model = %self.model, count = centroids.count(),
                        elapsed_ms = started.elapsed().as_millis() as u64, "loaded embedding centroids");
                    tokio::time::sleep(RELOAD_INTERVAL).await;
                }
                Err(err) => {
                    warn!(error = %format!("{err:#}"), "failed to load embedding centroids; retrying");
                    tokio::time::sleep(LOAD_RETRY_INTERVAL).await;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
