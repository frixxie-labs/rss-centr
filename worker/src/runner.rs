use anyhow::Result;
use metrics::{counter, gauge};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, DequeuedFeedUpdate, FailedFeedUpdateRequest,
};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::feed_fetcher::{FetchOutcome, fetch_feed};
use crate::feed_mapper::{feed_title_and_site_url, feed_to_items};
use crate::queue_client::QueueClient;
use crate::telemetry::record_feed_processed;

pub(crate) async fn run_once(
    queue: &QueueClient,
    http: &reqwest::Client,
    limit: i64,
    lease_seconds: i64,
) -> Result<usize> {
    let feeds = queue.dequeue(limit, lease_seconds).await?;
    // Set (not just recorded on activity) so the gauge drops back to 0 once
    // the queue drains, rather than sticking at the last nonzero batch size.
    gauge!("rss_centr_worker_dequeued_feeds").set(feeds.len() as f64);
    if feeds.is_empty() {
        return Ok(0);
    }

    let count = feeds.len();
    let mut tasks = JoinSet::new();
    for feed in feeds {
        let queue = queue.clone();
        let http = http.clone();
        let feed_id = feed.feed_id;
        tasks.spawn(async move { (feed_id, process_feed(&queue, &http, feed).await) });
    }

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((feed_id, Ok(()))) => {
                info!(feed_id = feed_id, "feed update task finished");
            }
            Ok((feed_id, Err(e))) => {
                warn!(feed_id = feed_id, error = %e, "feed update failed");
            }
            Err(e) => {
                warn!(error = %e, "feed update task panicked");
            }
        }
    }

    Ok(count)
}

async fn process_feed(
    queue: &QueueClient,
    http: &reqwest::Client,
    feed: DequeuedFeedUpdate,
) -> Result<()> {
    let feed_id = feed.feed_id;
    let lease_token = feed.lease_token.clone();

    match fetch_feed(http, &feed).await {
        Ok(FetchOutcome::NotModified {
            etag,
            last_modified,
        }) => {
            let result = queue
                .complete(
                    feed_id,
                    CompleteFeedUpdateRequest {
                        lease_token,
                        fetched: false,
                        title: None,
                        site_url: None,
                        etag,
                        last_modified,
                        items: Vec::new(),
                    },
                )
                .await?;
            record_feed_processed("not_modified");
            info!(
                feed_id = feed_id,
                inserted_items = result.inserted_items,
                next_due_at = %result.next_due_at,
                "feed not modified"
            );
        }
        Ok(FetchOutcome::Fetched {
            feed,
            etag,
            last_modified,
        }) => {
            let items = feed_to_items(&feed);
            let item_count = items.len();
            let (title, site_url) = feed_title_and_site_url(&feed);
            let result = queue
                .complete(
                    feed_id,
                    CompleteFeedUpdateRequest {
                        lease_token,
                        fetched: true,
                        title,
                        site_url,
                        etag,
                        last_modified,
                        items,
                    },
                )
                .await?;
            record_feed_processed("fetched");
            counter!("rss_centr_worker_feed_items_inserted_total")
                .increment(result.inserted_items as u64);
            info!(
                feed_id = feed_id,
                fetched_items = item_count,
                inserted_items = result.inserted_items,
                next_due_at = %result.next_due_at,
                "feed update completed"
            );
        }
        Err(fetch_error) => {
            record_feed_processed("failed");
            let failed = queue
                .failed(feed_id, FailedFeedUpdateRequest { lease_token })
                .await;
            match failed {
                Ok(result) => {
                    warn!(
                        feed_id = feed_id,
                        next_due_at = %result.next_due_at,
                        error = %fetch_error,
                        "feed update rescheduled after fetch failure"
                    );
                }
                Err(fail_error) => {
                    error!(
                        feed_id = feed_id,
                        fetch_error = %fetch_error,
                        fail_error = %fail_error,
                        "failed to record feed update failure"
                    );
                    return Err(fail_error);
                }
            }
        }
    }

    Ok(())
}
