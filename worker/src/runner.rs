use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use metrics::{counter, gauge};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, DequeuedFeedUpdate, FailedFeedUpdateRequest,
};
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep_until};
use tracing::{error, info, warn};

use crate::feed_fetcher::{FeedSource, FetchError, FetchOutcome};
use crate::feed_mapper::{feed_title_and_site_url, feed_to_items};
use crate::http_util::truncate_chars;
use crate::queue_client::FeedQueue;
use crate::telemetry::record_feed_processed;

/// Longest fetch error description sent to the backend.
const MAX_FAILURE_ERROR_CHARS: usize = 1_000;

#[derive(Debug, Clone, Copy)]
pub(crate) struct RunnerConfig {
    pub(crate) limit: u32,
    pub(crate) lease_seconds: u32,
    pub(crate) idle_sleep: Duration,
}

type FeedTasks = JoinSet<(i64, Result<()>)>;

/// Dequeue one batch and process it to completion. Returns the number of
/// feeds dequeued.
pub(crate) async fn run_once<Q: FeedQueue, S: FeedSource>(
    queue: Arc<Q>,
    source: Arc<S>,
    config: RunnerConfig,
) -> Result<usize> {
    let feeds = queue.dequeue(config.limit, config.lease_seconds).await?;
    gauge!("rss_centr_worker_dequeued_feeds").set(feeds.len() as f64);
    let count = feeds.len();
    let mut tasks = FeedTasks::new();
    spawn_feeds(&mut tasks, &queue, &source, feeds);
    while let Some(result) = tasks.join_next().await {
        log_task_result(result);
    }
    Ok(count)
}

/// Keeps up to `config.limit` feeds in flight, refilling free slots as
/// soon as feeds finish. When `shutdown` flips to `true` (or its sender is
/// dropped) no new feeds are dequeued and in-flight feeds are finished
/// before returning, so their leases are released instead of expiring.
pub(crate) async fn run<Q: FeedQueue, S: FeedSource>(
    queue: Arc<Q>,
    source: Arc<S>,
    config: RunnerConfig,
    mut shutdown: watch::Receiver<bool>,
) {
    let limit = config.limit as usize;
    let mut tasks = FeedTasks::new();
    let mut next_poll = Instant::now();
    let mut stopping = *shutdown.borrow();

    loop {
        if stopping && tasks.is_empty() {
            break;
        }
        let free = limit.saturating_sub(tasks.len());
        if !stopping && free > 0 && Instant::now() >= next_poll {
            match queue.dequeue(free as u32, config.lease_seconds).await {
                Ok(feeds) => {
                    gauge!("rss_centr_worker_dequeued_feeds").set(feeds.len() as f64);
                    if feeds.len() < free {
                        // The queue is drained; wait before asking again.
                        next_poll = Instant::now() + config.idle_sleep;
                    }
                    spawn_feeds(&mut tasks, &queue, &source, feeds);
                }
                Err(e) => {
                    warn!(
                        error = format!("{e:#}"),
                        "failed to dequeue feed updates; retrying after idle sleep"
                    );
                    next_poll = Instant::now() + config.idle_sleep;
                }
            }
            continue;
        }

        let can_poll = !stopping && tasks.len() < limit;
        tokio::select! {
            Some(result) = tasks.join_next(), if !tasks.is_empty() => log_task_result(result),
            () = sleep_until(next_poll), if can_poll => {}
            _ = shutdown.changed(), if !stopping => {
                stopping = true;
                info!(in_flight = tasks.len(), "shutting down; finishing in-flight feeds");
            }
        }
    }
}

fn spawn_feeds<Q: FeedQueue, S: FeedSource>(
    tasks: &mut FeedTasks,
    queue: &Arc<Q>,
    source: &Arc<S>,
    feeds: Vec<DequeuedFeedUpdate>,
) {
    for feed in feeds {
        let queue = Arc::clone(queue);
        let source = Arc::clone(source);
        let feed_id = feed.feed_id;
        tasks.spawn(async move { (feed_id, process_feed(&*queue, &*source, feed).await) });
    }
}

fn log_task_result(result: Result<(i64, Result<()>), tokio::task::JoinError>) {
    match result {
        Ok((feed_id, Ok(()))) => info!(feed_id, "feed update task finished"),
        Ok((feed_id, Err(e))) => warn!(feed_id, error = format!("{e:#}"), "feed update failed"),
        Err(e) => error!(error = %e, "feed update task panicked"),
    }
}

async fn process_feed(
    queue: &impl FeedQueue,
    source: &impl FeedSource,
    feed: DequeuedFeedUpdate,
) -> Result<()> {
    let feed_id = feed.feed_id;
    let lease_token = feed.lease_token.clone();

    match source.fetch(&feed).await {
        Ok(outcome) => {
            let request = completion_request(lease_token, &outcome);
            let fetched = request.fetched;
            let fetched_items = request.items.len();
            let result = queue.complete(feed_id, request).await?;
            if fetched {
                record_feed_processed("fetched");
                counter!("rss_centr_worker_feed_items_inserted_total")
                    .increment(result.inserted_items as u64);
                info!(
                    feed_id,
                    fetched_items,
                    inserted_items = result.inserted_items,
                    next_due_at = %result.next_due_at,
                    "feed update completed"
                );
            } else {
                record_feed_processed("not_modified");
                info!(
                    feed_id,
                    inserted_items = result.inserted_items,
                    next_due_at = %result.next_due_at,
                    "feed not modified"
                );
            }
        }
        Err(fetch_error) => {
            record_feed_processed("failed");
            let request = failure_request(lease_token, &fetch_error);
            let retry_after = request.retry_after;
            let http_status = request.http_status;
            match queue.failed(feed_id, request).await {
                Ok(result) => {
                    warn!(
                        feed_id,
                        next_due_at = %result.next_due_at,
                        blocked_by_bot_protection = fetch_error.blocked_by_bot_protection(),
                        http_status,
                        retry_after = ?retry_after,
                        error = %fetch_error,
                        "feed update rescheduled after fetch failure"
                    );
                }
                Err(fail_error) => {
                    error!(
                        feed_id,
                        fetch_error = %fetch_error,
                        fail_error = format!("{fail_error:#}"),
                        "failed to record feed update failure"
                    );
                    return Err(fail_error);
                }
            }
        }
    }

    Ok(())
}

fn completion_request(lease_token: String, outcome: &FetchOutcome) -> CompleteFeedUpdateRequest {
    match outcome {
        FetchOutcome::NotModified {
            etag,
            last_modified,
        } => CompleteFeedUpdateRequest {
            lease_token,
            fetched: false,
            title: None,
            site_url: None,
            etag: etag.clone(),
            last_modified: last_modified.clone(),
            items: Vec::new(),
        },
        FetchOutcome::Fetched {
            feed,
            etag,
            last_modified,
        } => {
            let (title, site_url) = feed_title_and_site_url(feed);
            CompleteFeedUpdateRequest {
                lease_token,
                fetched: true,
                title,
                site_url,
                etag: etag.clone(),
                last_modified: last_modified.clone(),
                items: feed_to_items(feed),
            }
        }
    }
}

fn failure_request(lease_token: String, fetch_error: &FetchError) -> FailedFeedUpdateRequest {
    let error = fetch_error.to_string();
    FailedFeedUpdateRequest {
        lease_token,
        retry_after: fetch_error.retry_after(),
        http_status: fetch_error.http_status(),
        error: Some(truncate_chars(&error, MAX_FAILURE_ERROR_CHARS).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use anyhow::anyhow;
    use chrono::Utc;
    use reqwest::StatusCode;
    use rss_centr_core::feed_update_queue::{CompleteFeedUpdateResult, FailedFeedUpdateResult};

    use super::*;
    use crate::feed_fetcher::{FeedFetchFailure, FetchFailureReason};

    fn feed(feed_id: i64) -> DequeuedFeedUpdate {
        DequeuedFeedUpdate {
            feed_id,
            url: format!("https://example.com/{feed_id}.xml"),
            title: None,
            site_url: None,
            etag: None,
            last_modified: None,
            poll_interval_seconds: 300,
            last_checked_at: None,
            last_success_at: None,
            last_inserted_at: None,
            failure_count: 0,
            lease_token: format!("lease-{feed_id}"),
            lease_expires_at: Utc::now(),
        }
    }

    #[derive(Default)]
    struct FakeQueue {
        batches: Mutex<VecDeque<Vec<DequeuedFeedUpdate>>>,
        dequeue_limits: Mutex<Vec<u32>>,
        completed: Mutex<Vec<(i64, CompleteFeedUpdateRequest)>>,
        failed: Mutex<Vec<(i64, FailedFeedUpdateRequest)>>,
    }

    impl FakeQueue {
        fn with_batches(batches: Vec<Vec<DequeuedFeedUpdate>>) -> Self {
            Self {
                batches: Mutex::new(batches.into()),
                ..Default::default()
            }
        }
    }

    impl FeedQueue for FakeQueue {
        async fn dequeue(&self, limit: u32, _lease: u32) -> Result<Vec<DequeuedFeedUpdate>> {
            self.dequeue_limits.lock().unwrap().push(limit);
            Ok(self.batches.lock().unwrap().pop_front().unwrap_or_default())
        }

        async fn complete(
            &self,
            feed_id: i64,
            request: CompleteFeedUpdateRequest,
        ) -> Result<CompleteFeedUpdateResult> {
            let inserted_items = request.items.len() as i64;
            self.completed.lock().unwrap().push((feed_id, request));
            Ok(CompleteFeedUpdateResult {
                inserted_items,
                next_due_at: Utc::now(),
            })
        }

        async fn failed(
            &self,
            feed_id: i64,
            request: FailedFeedUpdateRequest,
        ) -> Result<FailedFeedUpdateResult> {
            self.failed.lock().unwrap().push((feed_id, request));
            Ok(FailedFeedUpdateResult {
                next_due_at: Utc::now(),
            })
        }
    }

    /// Feeds with even ids are not modified; odd ids fail with HTTP 410.
    #[derive(Default)]
    struct FakeSource {
        delay: Duration,
        fetches: AtomicUsize,
    }

    impl FeedSource for FakeSource {
        async fn fetch(&self, feed: &DequeuedFeedUpdate) -> Result<FetchOutcome, FetchError> {
            tokio::time::sleep(self.delay).await;
            self.fetches.fetch_add(1, Ordering::SeqCst);
            if feed.feed_id % 2 == 0 {
                Ok(FetchOutcome::NotModified {
                    etag: Some("e".to_string()),
                    last_modified: None,
                })
            } else {
                Err(FetchError::Rejected {
                    failure: FeedFetchFailure {
                        status: StatusCode::GONE,
                        reason: FetchFailureReason::HttpStatus,
                        retry_after: None,
                    },
                    source: None,
                })
            }
        }
    }

    fn config(limit: u32) -> RunnerConfig {
        RunnerConfig {
            limit,
            lease_seconds: 60,
            idle_sleep: Duration::from_secs(30),
        }
    }

    #[tokio::test]
    async fn run_once_completes_and_fails_feeds() {
        let queue = Arc::new(FakeQueue::with_batches(vec![vec![feed(2), feed(3)]]));
        let source = Arc::new(FakeSource::default());

        let count = run_once(Arc::clone(&queue), source, config(5))
            .await
            .unwrap();

        assert_eq!(count, 2);
        let completed = queue.completed.lock().unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].0, 2);
        assert!(!completed[0].1.fetched);
        assert_eq!(completed[0].1.lease_token, "lease-2");
        assert_eq!(completed[0].1.etag.as_deref(), Some("e"));

        let failed = queue.failed.lock().unwrap();
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].0, 3);
        assert_eq!(failed[0].1.lease_token, "lease-3");
        assert_eq!(failed[0].1.http_status, Some(410));
        let error = failed[0].1.error.as_deref().unwrap();
        assert!(error.contains("HTTP 410"), "{error}");
    }

    #[test]
    fn completion_request_carries_validators_and_only_fetched_feeds_carry_content() {
        let feed = crate::feed_parser::parse_feed(
            br#"<rss version="2.0"><channel><title>Feed</title><link>https://example.com</link><item><title>a</title><guid>1</guid></item></channel></rss>"#,
        )
        .unwrap();
        let fetched = completion_request(
            "lease".to_string(),
            &FetchOutcome::Fetched {
                feed: Box::new(feed),
                etag: Some("e".to_string()),
                last_modified: Some("m".to_string()),
            },
        );
        assert!(fetched.fetched);
        assert_eq!(fetched.lease_token, "lease");
        assert_eq!(fetched.title.as_deref(), Some("Feed"));
        assert_eq!(fetched.items.len(), 1);
        assert_eq!(
            (fetched.etag.as_deref(), fetched.last_modified.as_deref()),
            (Some("e"), Some("m"))
        );

        let not_modified = completion_request(
            "lease".to_string(),
            &FetchOutcome::NotModified {
                etag: Some("e".to_string()),
                last_modified: None,
            },
        );
        assert!(!not_modified.fetched);
        assert!(not_modified.title.is_none() && not_modified.items.is_empty());
        assert_eq!(not_modified.etag.as_deref(), Some("e"));
    }

    #[test]
    fn failure_request_truncates_long_errors() {
        let error = FetchError::Other(anyhow!("x".repeat(MAX_FAILURE_ERROR_CHARS + 50)));
        let request = failure_request("t".to_string(), &error);
        assert_eq!(request.http_status, None);
        assert_eq!(
            request.error.unwrap().chars().count(),
            MAX_FAILURE_ERROR_CHARS
        );
    }

    #[tokio::test(start_paused = true)]
    async fn run_refills_free_slots_and_finishes_in_flight_feeds_on_shutdown() {
        let queue = Arc::new(FakeQueue::with_batches(vec![
            vec![feed(2), feed(4)],
            vec![feed(6)],
        ]));
        let source = Arc::new(FakeSource {
            delay: Duration::from_secs(1),
            ..Default::default()
        });
        let (stop, shutdown) = watch::channel(false);
        let runner = tokio::spawn(run(
            Arc::clone(&queue),
            Arc::clone(&source),
            config(2),
            shutdown,
        ));

        // The first batch fills both slots. Each finished feed frees one
        // slot, which is refilled right away: first with feed 6, then with
        // nothing (queue drained), after which the runner idles.
        tokio::time::sleep(Duration::from_millis(1_500)).await;
        stop.send(true).unwrap();
        runner.await.unwrap();

        assert_eq!(source.fetches.load(Ordering::SeqCst), 3);
        assert_eq!(queue.completed.lock().unwrap().len(), 3);
        assert_eq!(*queue.dequeue_limits.lock().unwrap(), vec![2, 1, 1]);
    }

    #[tokio::test(start_paused = true)]
    async fn run_stops_dequeuing_after_shutdown() {
        let queue = Arc::new(FakeQueue::with_batches(vec![vec![feed(2)]; 10]));
        let source = Arc::new(FakeSource::default());
        let (stop, shutdown) = watch::channel(true);

        run(Arc::clone(&queue), source, config(2), shutdown).await;

        assert!(queue.dequeue_limits.lock().unwrap().is_empty());
        drop(stop);
    }
}
