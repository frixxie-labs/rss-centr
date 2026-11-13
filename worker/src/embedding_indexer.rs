use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use metrics::counter;
use rillfp::AsyncReaderResult;
use rss_centr_core::ollama;
use rss_centr_core::semantic::{
    ClaimEmbeddingsResponse, ItemEmbedding, StoreEmbeddingsRequest, StoreEmbeddingsResult,
};
use tokio::sync::Notify;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::http_util::send_json;

/// Maximum number of documents sent to Ollama in one request.
const EMBED_BATCH_SIZE: usize = 16;
const CLAIM_LIMIT: i64 = 64;
const LEASE_SECONDS: i64 = 600;
// Fallback for items inserted by other workers, failed batches and model
// changes; new items from this worker wake the indexer immediately.
const IDLE_SLEEP: Duration = Duration::from_secs(15 * 60);
const RETRY_SLEEP: Duration = Duration::from_secs(60);

pub(crate) trait EmbeddingQueue: Send + Sync {
    fn claim(
        &self,
        limit: i64,
        lease_seconds: i64,
    ) -> impl Future<Output = Result<ClaimEmbeddingsResponse>> + Send;
    fn store(
        &self,
        request: StoreEmbeddingsRequest,
    ) -> impl Future<Output = Result<StoreEmbeddingsResult>> + Send;
}

pub(crate) trait DocumentEmbedder: Send + Sync {
    fn embed(
        &self,
        model: &str,
        inputs: &[String],
    ) -> impl Future<Output = Result<Vec<Vec<f32>>>> + Send;
}

struct EmbeddingEnvironment<Q, E> {
    queue: Q,
    embedder: E,
}

/// Build a reusable batch: every run claims fresh work, then embeds and stores
/// it sequentially. Dropping a run leaves any claimed leases to expire.
fn run_once<Q: EmbeddingQueue + 'static, E: DocumentEmbedder + 'static>(
    limit: i64,
    lease_seconds: i64,
) -> AsyncReaderResult<EmbeddingEnvironment<Q, E>, usize, anyhow::Error> {
    AsyncReaderResult::new(move |environment: &EmbeddingEnvironment<Q, E>| {
        Box::pin(environment.queue.claim(limit, lease_seconds))
    })
    .and_then_async(|claimed, environment| {
        Box::pin(async move { embed_and_store(environment, &claimed).await })
    })
}

async fn embed_and_store<Q: EmbeddingQueue, E: DocumentEmbedder>(
    environment: &EmbeddingEnvironment<Q, E>,
    claimed: &ClaimEmbeddingsResponse,
) -> Result<usize> {
    let count = claimed.jobs.len();
    if count == 0 {
        return Ok(0);
    }

    let mut embeddings = Vec::with_capacity(count);
    for chunk in claimed.jobs.chunks(EMBED_BATCH_SIZE) {
        let inputs: Vec<String> = chunk.iter().map(|job| job.input.clone()).collect();
        let vectors = environment
            .embedder
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

    let result = environment
        .queue
        .store(StoreEmbeddingsRequest {
            model: claimed.model.clone(),
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
    new_items: Arc<Notify>,
) {
    let queue = HttpEmbeddingQueue {
        base_url: backend_url.trim_end_matches('/').to_string(),
        http,
    };
    let embedder = OllamaEmbedder {
        ollama_url,
        http: ollama_http,
    };
    let environment = EmbeddingEnvironment { queue, embedder };
    let batch = run_once(CLAIM_LIMIT, LEASE_SECONDS);
    loop {
        match batch.run(&environment).await {
            Ok(0) => wait_for_new_items(&new_items, IDLE_SLEEP).await,
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

/// Waits until new feed items were inserted, or at most `timeout` so items
/// from other workers and model changes are still picked up.
async fn wait_for_new_items(new_items: &Notify, timeout: Duration) {
    let _ = tokio::time::timeout(timeout, new_items.notified()).await;
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

    #[tokio::test(start_paused = true)]
    async fn wait_ends_when_new_items_arrive() {
        let new_items = Notify::new();
        new_items.notify_one();
        let started = tokio::time::Instant::now();
        wait_for_new_items(&new_items, IDLE_SLEEP).await;
        assert_eq!(started.elapsed(), Duration::ZERO);
    }

    #[tokio::test(start_paused = true)]
    async fn wait_falls_back_to_timeout() {
        let new_items = Notify::new();
        let started = tokio::time::Instant::now();
        wait_for_new_items(&new_items, IDLE_SLEEP).await;
        assert_eq!(started.elapsed(), IDLE_SLEEP);
    }

    #[derive(Default)]
    struct FakeQueue {
        jobs: Vec<EmbeddingJob>,
        stored: Mutex<Vec<StoreEmbeddingsRequest>>,
        claims: Mutex<Vec<(i64, i64)>>,
        fail_claim: bool,
        fail_store: bool,
    }

    impl EmbeddingQueue for FakeQueue {
        async fn claim(&self, limit: i64, lease: i64) -> Result<ClaimEmbeddingsResponse> {
            self.claims.lock().unwrap().push((limit, lease));
            if self.fail_claim {
                anyhow::bail!("claim unavailable");
            }
            Ok(ClaimEmbeddingsResponse {
                model: "m".to_string(),
                jobs: self.jobs.iter().take(limit as usize).cloned().collect(),
            })
        }

        async fn store(&self, request: StoreEmbeddingsRequest) -> Result<StoreEmbeddingsResult> {
            if self.fail_store {
                anyhow::bail!("store unavailable");
            }
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
        fail_on_batch: Option<usize>,
        release: Option<Arc<Notify>>,
    }

    impl DocumentEmbedder for FakeEmbedder {
        async fn embed(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
            assert_eq!(model, "m");
            if self.fail {
                anyhow::bail!("ollama down");
            }
            self.batches.lock().unwrap().push(inputs.len());
            if self.fail_on_batch == Some(self.batches.lock().unwrap().len()) {
                anyhow::bail!("later batch failed");
            }
            if let Some(release) = &self.release {
                release.notified().await;
            }
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
        let environment = EmbeddingEnvironment {
            queue: FakeQueue::default(),
            embedder: FakeEmbedder::default(),
        };
        assert_eq!(run_once(10, 60).run(&environment).await.unwrap(), 0);
        assert!(environment.queue.stored.lock().unwrap().is_empty());
        assert!(environment.embedder.batches.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn embeds_claimed_items_in_batches_and_stores_them_in_order() {
        let queue = FakeQueue {
            jobs: jobs(20),
            ..Default::default()
        };
        let embedder = FakeEmbedder::default();
        let environment = EmbeddingEnvironment { queue, embedder };

        assert_eq!(run_once(64, 60).run(&environment).await.unwrap(), 20);
        assert_eq!(*environment.embedder.batches.lock().unwrap(), vec![16, 4]);

        let stored = environment.queue.stored.lock().unwrap();
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
        let environment = EmbeddingEnvironment { queue, embedder };
        assert!(run_once(10, 60).run(&environment).await.is_err());
        assert!(environment.queue.stored.lock().unwrap().is_empty());
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
        let environment = EmbeddingEnvironment { queue, embedder };
        let err = run_once(10, 60).run(&environment).await.unwrap_err();
        assert!(err.to_string().contains("2 vectors for 3 inputs"));
        assert!(environment.queue.stored.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn embedding_workflow_is_lazy_send_and_reclaims_on_each_run() {
        let environment = EmbeddingEnvironment {
            queue: FakeQueue {
                jobs: jobs(3),
                ..Default::default()
            },
            embedder: FakeEmbedder::default(),
        };
        let workflow = run_once(10, 60);
        let unpolled = workflow.run(&environment);
        assert!(environment.queue.claims.lock().unwrap().is_empty());
        drop(unpolled);
        fn assert_send<T: Send>(value: T) -> T {
            value
        }
        for _ in 0..2 {
            assert_eq!(assert_send(workflow.run(&environment)).await.unwrap(), 3);
        }
        assert_eq!(
            *environment.queue.claims.lock().unwrap(),
            [(10, 60), (10, 60)]
        );
        assert_eq!(environment.queue.stored.lock().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn claim_failure_skips_embedding_and_storage() {
        let environment = EmbeddingEnvironment {
            queue: FakeQueue {
                fail_claim: true,
                ..Default::default()
            },
            embedder: FakeEmbedder::default(),
        };
        let error = run_once(10, 60).run(&environment).await.unwrap_err();
        assert_eq!(error.to_string(), "claim unavailable");
        assert!(environment.embedder.batches.lock().unwrap().is_empty());
        assert!(environment.queue.stored.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn later_batch_failure_does_not_store_partial_embeddings() {
        let environment = EmbeddingEnvironment {
            queue: FakeQueue {
                jobs: jobs(40),
                ..Default::default()
            },
            embedder: FakeEmbedder {
                fail_on_batch: Some(2),
                ..Default::default()
            },
        };
        let error = run_once(64, 60).run(&environment).await.unwrap_err();
        assert!(format!("{error:#}").contains("later batch failed"));
        assert_eq!(*environment.embedder.batches.lock().unwrap(), [16, 16]);
        assert!(environment.queue.stored.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn storage_failure_is_propagated_without_retrying() {
        let environment = EmbeddingEnvironment {
            queue: FakeQueue {
                jobs: jobs(3),
                fail_store: true,
                ..Default::default()
            },
            embedder: FakeEmbedder::default(),
        };
        let error = run_once(10, 60).run(&environment).await.unwrap_err();
        assert_eq!(error.to_string(), "store unavailable");
        assert_eq!(*environment.embedder.batches.lock().unwrap(), [3]);
        assert_eq!(*environment.queue.claims.lock().unwrap(), [(10, 60)]);
    }

    #[tokio::test(start_paused = true)]
    async fn cancelling_a_batch_stops_storage_and_does_not_poison_the_next_run() {
        let release = Arc::new(Notify::new());
        let environment = EmbeddingEnvironment {
            queue: FakeQueue {
                jobs: jobs(3),
                ..Default::default()
            },
            embedder: FakeEmbedder {
                release: Some(Arc::clone(&release)),
                ..Default::default()
            },
        };
        let batch = run_once(10, 60);
        assert!(
            tokio::time::timeout(Duration::from_secs(1), batch.run(&environment))
                .await
                .is_err()
        );
        assert_eq!(*environment.queue.claims.lock().unwrap(), [(10, 60)]);
        assert_eq!(*environment.embedder.batches.lock().unwrap(), [3]);
        assert!(environment.queue.stored.lock().unwrap().is_empty());

        release.notify_one();
        assert_eq!(batch.run(&environment).await.unwrap(), 3);
        assert_eq!(environment.queue.stored.lock().unwrap().len(), 1);
        assert_eq!(
            *environment.queue.claims.lock().unwrap(),
            [(10, 60), (10, 60)]
        );
    }
}
