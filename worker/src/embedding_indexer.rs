use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use metrics::counter;
use rss_centr_core::ollama;
use rss_centr_core::semantic::{
    ClaimEmbeddingsResponse, ItemEmbedding, StoreEmbeddingsRequest, StoreEmbeddingsResult,
};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::http_util::send_json;

/// Maximum number of documents sent to Ollama in one request.
const EMBED_BATCH_SIZE: usize = 16;
const CLAIM_LIMIT: i64 = 64;
const LEASE_SECONDS: i64 = 600;
// Feeds are polled at most about every 13 minutes, so new items rarely
// arrive more often than this.
const IDLE_SLEEP: Duration = Duration::from_secs(15 * 60);
const RETRY_SLEEP: Duration = Duration::from_secs(60);

pub(crate) trait EmbeddingQueue {
    async fn claim(&self, limit: i64, lease_seconds: i64) -> Result<ClaimEmbeddingsResponse>;
    async fn store(&self, request: StoreEmbeddingsRequest) -> Result<StoreEmbeddingsResult>;
}

pub(crate) trait DocumentEmbedder {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>>;
}

/// Claim one batch, embed it and store the result. Returns the number of
/// items claimed (0 when there is nothing to do).
pub(crate) async fn run_once(
    queue: &impl EmbeddingQueue,
    embedder: &impl DocumentEmbedder,
    limit: i64,
    lease_seconds: i64,
) -> Result<usize> {
    let claimed = queue.claim(limit, lease_seconds).await?;
    let count = claimed.jobs.len();
    if count == 0 {
        return Ok(0);
    }

    let mut embeddings = Vec::with_capacity(count);
    for chunk in claimed.jobs.chunks(EMBED_BATCH_SIZE) {
        let inputs: Vec<String> = chunk.iter().map(|job| job.input.clone()).collect();
        let vectors = embedder
            .embed(&claimed.model, &inputs)
            .await
            .with_context(|| format!("failed to embed {} items", chunk.len()))?;
        if vectors.len() != chunk.len() {
            return Err(anyhow!(
                "embedder returned {} vectors for {} inputs",
                vectors.len(),
                chunk.len()
            ));
        }
        embeddings.extend(
            chunk
                .iter()
                .zip(vectors)
                .map(|(job, embedding)| ItemEmbedding {
                    feed_item_id: job.feed_item_id,
                    content_hash: job.content_hash.clone(),
                    embedding,
                }),
        );
    }

    let result = queue
        .store(StoreEmbeddingsRequest {
            model: claimed.model,
            embeddings,
        })
        .await?;
    counter!("rss_centr_worker_embedded_items").increment(result.stored);
    Ok(count)
}

pub(crate) async fn run(
    backend_url: String,
    ollama_url: String,
    http: reqwest::Client,
    ollama_http: reqwest::Client,
) {
    let queue = HttpEmbeddingQueue {
        base_url: backend_url.trim_end_matches('/').to_string(),
        http,
    };
    let embedder = OllamaEmbedder {
        ollama_url,
        http: ollama_http,
    };
    loop {
        match run_once(&queue, &embedder, CLAIM_LIMIT, LEASE_SECONDS).await {
            Ok(0) => sleep(IDLE_SLEEP).await,
            Ok(count) => info!(count, "embedded feed items"),
            Err(err) => {
                warn!(
                    error = format!("{err:#}"),
                    "embedding batch failed; retrying"
                );
                sleep(RETRY_SLEEP).await;
            }
        }
    }
}

struct HttpEmbeddingQueue {
    base_url: String,
    http: reqwest::Client,
}

impl EmbeddingQueue for HttpEmbeddingQueue {
    async fn claim(&self, limit: i64, lease_seconds: i64) -> Result<ClaimEmbeddingsResponse> {
        let url = format!(
            "{}/internal/embeddings/claim?limit={limit}&lease_seconds={lease_seconds}",
            self.base_url
        );
        send_json(self.http.post(url), "claim embedding jobs").await
    }

    async fn store(&self, request: StoreEmbeddingsRequest) -> Result<StoreEmbeddingsResult> {
        let url = format!("{}/internal/embeddings", self.base_url);
        send_json(self.http.put(url).json(&request), "store embeddings").await
    }
}

struct OllamaEmbedder {
    ollama_url: String,
    http: reqwest::Client,
}

impl DocumentEmbedder for OllamaEmbedder {
    async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        ollama::embed(&self.http, &self.ollama_url, model, inputs, None).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use rss_centr_core::semantic::EmbeddingJob;

    #[derive(Default)]
    struct FakeQueue {
        jobs: Vec<EmbeddingJob>,
        stored: Mutex<Vec<StoreEmbeddingsRequest>>,
    }

    impl EmbeddingQueue for FakeQueue {
        async fn claim(&self, limit: i64, _lease: i64) -> Result<ClaimEmbeddingsResponse> {
            Ok(ClaimEmbeddingsResponse {
                model: "m".to_string(),
                jobs: self.jobs.iter().take(limit as usize).cloned().collect(),
            })
        }

        async fn store(&self, request: StoreEmbeddingsRequest) -> Result<StoreEmbeddingsResult> {
            let stored = request.embeddings.len() as u64;
            self.stored.lock().unwrap().push(request);
            Ok(StoreEmbeddingsResult { stored })
        }
    }

    /// Embeds each input as `[len]`, recording batch sizes.
    #[derive(Default)]
    struct FakeEmbedder {
        batches: Mutex<Vec<usize>>,
        fail: bool,
        drop_one: bool,
    }

    impl DocumentEmbedder for FakeEmbedder {
        async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
            assert_eq!(model, "m");
            if self.fail {
                anyhow::bail!("ollama down");
            }
            self.batches.lock().unwrap().push(inputs.len());
            let mut vectors: Vec<Vec<f32>> = inputs.iter().map(|i| vec![i.len() as f32]).collect();
            if self.drop_one {
                vectors.pop();
            }
            Ok(vectors)
        }
    }

    fn jobs(n: usize) -> Vec<EmbeddingJob> {
        (0..n)
            .map(|i| EmbeddingJob {
                feed_item_id: i as i64,
                input: "x".repeat(i + 1),
                content_hash: format!("h{i}"),
            })
            .collect()
    }

    #[tokio::test]
    async fn empty_queue_does_nothing() {
        let queue = FakeQueue::default();
        let embedder = FakeEmbedder::default();
        assert_eq!(run_once(&queue, &embedder, 10, 60).await.unwrap(), 0);
        assert!(queue.stored.lock().unwrap().is_empty());
        assert!(embedder.batches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn embeds_claimed_items_in_batches_and_stores_them_in_order() {
        let queue = FakeQueue {
            jobs: jobs(20),
            ..Default::default()
        };
        let embedder = FakeEmbedder::default();

        assert_eq!(run_once(&queue, &embedder, 64, 60).await.unwrap(), 20);
        assert_eq!(*embedder.batches.lock().unwrap(), vec![16, 4]);

        let stored = queue.stored.lock().unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].model, "m");
        let first = &stored[0].embeddings[0];
        assert_eq!(first.feed_item_id, 0);
        assert_eq!(first.content_hash, "h0");
        assert_eq!(first.embedding, vec![1.0]);
        assert_eq!(stored[0].embeddings[19].embedding, vec![20.0]);
    }

    #[tokio::test]
    async fn embedding_failure_stores_nothing() {
        let queue = FakeQueue {
            jobs: jobs(3),
            ..Default::default()
        };
        let embedder = FakeEmbedder {
            fail: true,
            ..Default::default()
        };
        assert!(run_once(&queue, &embedder, 10, 60).await.is_err());
        assert!(queue.stored.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn mismatched_vector_count_is_an_error() {
        let queue = FakeQueue {
            jobs: jobs(3),
            ..Default::default()
        };
        let embedder = FakeEmbedder {
            drop_one: true,
            ..Default::default()
        };
        let err = run_once(&queue, &embedder, 10, 60).await.unwrap_err();
        assert!(err.to_string().contains("2 vectors for 3 inputs"));
        assert!(queue.stored.lock().unwrap().is_empty());
    }
}
