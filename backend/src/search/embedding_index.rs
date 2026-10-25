use std::collections::HashMap;
use std::future::Future;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rss_centr_core::semantic::QuantizedVector;
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

#[cfg(test)]
mod tests;

pub(crate) struct StoredEmbedding {
    pub(crate) id: i64,
    pub(crate) feed_id: i64,
    /// Effective publication date, capped at item insertion.
    pub(crate) date: DateTime<Utc>,
    pub(crate) embedding: Vec<f32>,
    pub(crate) embedded_at: DateTime<Utc>,
}

/// Everything about a stored embedding except the vector itself.
pub(crate) struct EmbeddingVersion {
    pub(crate) id: i64,
    pub(crate) feed_id: i64,
    pub(crate) date: DateTime<Utc>,
    pub(crate) embedded_at: DateTime<Utc>,
}

/// Read-side contract for keeping one model's in-memory index current.
pub(crate) trait EmbeddingSource: Send + Sync {
    /// Read one model's rows, optionally restricted to `embedded_at > since`.
    fn read_embeddings(
        &self,
        model: &str,
        since: Option<DateTime<Utc>>,
    ) -> impl Future<Output = Result<Vec<StoredEmbedding>>> + Send;

    /// Read existing rows for this model and these IDs, in no particular order.
    fn read_embeddings_by_ids(
        &self,
        model: &str,
        ids: &[i64],
    ) -> impl Future<Output = Result<Vec<StoredEmbedding>>> + Send;

    /// Read every row's identity and metadata without transferring vectors.
    fn read_embedding_versions(
        &self,
        model: &str,
    ) -> impl Future<Output = Result<Vec<EmbeddingVersion>>> + Send;
}

/// What [`EmbeddingIndex::reconcile`] changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconcileStats {
    /// Items dropped because their embedding is gone.
    pub removed: usize,
    /// Embeddings fetched because they were missing or outdated.
    pub fetched: usize,
}

/// Re-read a window to pick up rows committed late with an earlier `embedded_at`.
const REFRESH_OVERLAP: Duration = Duration::from_secs(5 * 60);
const RECONCILE_INTERVAL: Duration = Duration::from_secs(30 * 60);
const LOAD_RETRY_INTERVAL: Duration = Duration::from_secs(30);

struct IndexedItem {
    id: i64,
    feed_id: i64,
    /// Date used for recency ranking, see [`rss_centr_core::semantic::item_date`].
    date: DateTime<Utc>,
    embedded_at: DateTime<Utc>,
    vector: QuantizedVector,
}

/// A contiguous scan is faster than visiting a map's scattered entries.
#[derive(Default)]
struct IndexedItems {
    items: Vec<IndexedItem>,
    /// feed_item_id -> position in `items`
    positions: HashMap<i64, usize>,
}

impl IndexedItems {
    /// Insert or replace a row. Degenerate vectors are skipped.
    fn upsert(&mut self, row: StoredEmbedding) {
        let Some(vector) = QuantizedVector::new(&row.embedding) else {
            return;
        };
        let item = IndexedItem {
            id: row.id,
            feed_id: row.feed_id,
            date: row.date,
            embedded_at: row.embedded_at,
            vector,
        };
        match self.positions.get(&row.id) {
            Some(&position) => self.items[position] = item,
            None => {
                self.positions.insert(row.id, self.items.len());
                self.items.push(item);
            }
        }
    }

    fn get(&self, id: i64) -> Option<&IndexedItem> {
        self.positions
            .get(&id)
            .map(|&position| &self.items[position])
    }

    fn get_mut(&mut self, id: i64) -> Option<&mut IndexedItem> {
        self.positions
            .get(&id)
            .map(|&position| &mut self.items[position])
    }

    fn retain(&mut self, mut keep: impl FnMut(&IndexedItem) -> bool) {
        self.items.retain(|item| keep(item));
        self.positions = self
            .items
            .iter()
            .enumerate()
            .map(|(position, item)| (item.id, position))
            .collect();
    }

    fn iter(&self) -> impl Iterator<Item = &IndexedItem> {
        self.items.iter()
    }

    fn len(&self) -> usize {
        self.items.len()
    }
}

#[derive(Default)]
struct IndexState {
    items: IndexedItems,
    newest: Option<DateTime<Utc>>,
    loaded_at: Option<Instant>,
}

/// In-memory copy of one model's embeddings, quantized to a quarter of their size.
/// Searches pull recent updates; reconciliation also removes deleted embeddings.
pub struct EmbeddingIndex {
    model: String,
    state: RwLock<IndexState>,
    refresh: Mutex<()>,
}

impl EmbeddingIndex {
    pub fn new(model: impl Into<String>) -> Self {
        Self {
            model: model.into(),
            state: RwLock::new(IndexState::default()),
            refresh: Mutex::new(()),
        }
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    /// Pull embeddings stored since the last refresh; call before searching.
    /// The first call loads everything; concurrent callers wait for it.
    pub(crate) async fn refresh(&self, source: &impl EmbeddingSource) -> Result<()> {
        let _guard = self.refresh.lock().await;
        let (since, loaded) = {
            let state = self.state.read().await;
            let since = state
                .newest
                .map(|t| t - chrono::Duration::from_std(REFRESH_OVERLAP).unwrap_or_default());
            (since, state.loaded_at.is_some())
        };
        if !loaded {
            return self.load_all(source).await;
        }

        let rows = source.read_embeddings(&self.model, since).await?;
        let newest = rows.iter().map(|r| r.embedded_at).max();
        let mut state = self.state.write().await;
        for row in rows {
            state.items.upsert(row);
        }
        state.newest = state.newest.max(newest);
        Ok(())
    }

    /// Read every embedding and swap it in. Searches keep using the previous
    /// copy while this runs; refresh overlap picks up rows stored meanwhile.
    async fn load_all(&self, source: &impl EmbeddingSource) -> Result<()> {
        let rows = source.read_embeddings(&self.model, None).await?;
        let newest = rows.iter().map(|r| r.embedded_at).max();
        let mut items = IndexedItems::default();
        for row in rows {
            items.upsert(row);
        }
        let mut state = self.state.write().await;
        state.items = items;
        state.newest = newest;
        state.loaded_at = Some(Instant::now());
        Ok(())
    }

    /// Drop deleted embeddings, fetch missing/outdated vectors, and update dates.
    pub(crate) async fn reconcile(&self, source: &impl EmbeddingSource) -> Result<ReconcileStats> {
        let _guard = self.refresh.lock().await;
        let versions: HashMap<i64, EmbeddingVersion> = source
            .read_embedding_versions(&self.model)
            .await?
            .into_iter()
            .map(|version| (version.id, version))
            .collect();
        let missing: Vec<i64> = {
            let state = self.state.read().await;
            versions
                .values()
                .filter(|version| {
                    state
                        .items
                        .get(version.id)
                        .is_none_or(|item| item.embedded_at != version.embedded_at)
                })
                .map(|version| version.id)
                .collect()
        };
        let rows = if missing.is_empty() {
            Vec::new()
        } else {
            source.read_embeddings_by_ids(&self.model, &missing).await?
        };

        let mut state = self.state.write().await;
        let before = state.items.len();
        state.items.retain(|item| versions.contains_key(&item.id));
        let removed = before - state.items.len();
        for version in versions.values() {
            if let Some(item) = state.items.get_mut(version.id) {
                item.feed_id = version.feed_id;
                item.date = version.date;
            }
        }
        let fetched = rows.len();
        let newest = rows.iter().map(|r| r.embedded_at).max();
        for row in rows {
            state.items.upsert(row);
        }
        state.newest = state.newest.max(newest);
        Ok(ReconcileStats { removed, fetched })
    }

    /// Load immediately, then reconcile periodically. Failed loads are retried.
    pub(crate) async fn run_loader(&self, source: &impl EmbeddingSource) {
        let mut first = true;
        loop {
            let started = Instant::now();
            let result = if first {
                self.refresh(source).await.map(|()| None)
            } else {
                self.reconcile(source).await.map(Some)
            };
            match result {
                Ok(stats) => {
                    first = false;
                    let count = self.item_count().await;
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    match stats {
                        None => {
                            info!(model = %self.model, count, elapsed_ms, "loaded embedding index")
                        }
                        Some(stats) => info!(model = %self.model, count, removed = stats.removed,
                            fetched = stats.fetched, elapsed_ms, "reconciled embedding index"),
                    }
                    tokio::time::sleep(RECONCILE_INTERVAL).await;
                }
                Err(err) => {
                    warn!(error = %format!("{err:#}"), "failed to load embedding index; retrying");
                    tokio::time::sleep(LOAD_RETRY_INTERVAL).await;
                }
            }
        }
    }

    /// Score all indexed items using quantized cosine similarity. Other feeds
    /// and vector dimensions are skipped. Does not refresh the index.
    pub async fn similarities(&self, query: &[f32], feed_id: Option<i64>) -> Vec<(i64, f32)> {
        let Some(query) = QuantizedVector::new(query) else {
            return Vec::new();
        };
        let state = self.state.read().await;
        state
            .items
            .iter()
            .filter(|item| {
                feed_id.is_none_or(|f| f == item.feed_id) && item.vector.len() == query.len()
            })
            .map(|item| (item.id, query.dot(&item.vector)))
            .collect()
    }

    pub async fn item_count(&self) -> usize {
        self.state.read().await.items.len()
    }

    /// Unknown ids are skipped.
    pub async fn dates(&self, ids: impl IntoIterator<Item = i64>) -> HashMap<i64, DateTime<Utc>> {
        let state = self.state.read().await;
        ids.into_iter()
            .filter_map(|id| state.items.get(id).map(|item| (id, item.date)))
            .collect()
    }
}
