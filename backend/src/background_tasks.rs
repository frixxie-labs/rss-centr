use std::time::Duration;

use chrono::Utc;
use metrics::{counter, gauge};
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::sync::mpsc::{Receiver, Sender};
use tracing::{info, warn};

use crate::events::NewFeedItemEvent;
use crate::feed::{feed_subscription, ingest::ingest_feed_url};

#[derive(Debug, Clone)]
pub enum IngestJob {
    FeedId(i64),
    Url(String),
}

pub async fn handle_ingest_bg_thread(
    mut rx: Receiver<IngestJob>,
    pool: SqlitePool,
    client: reqwest::Client,
    new_item_tx: broadcast::Sender<NewFeedItemEvent>,
) {
    while let Some(job) = rx.recv().await {
        gauge!("rss_centr_ingest_queue_len").set(rx.len() as f64);

        let (feed_id, url) = match job {
            IngestJob::FeedId(feed_id) => {
                let feed = match feed_subscription::read_feed(&pool, feed_id).await {
                    Ok(feed) => feed,
                    Err(e) => {
                        counter!("rss_centr_ingest_errors_total").increment(1);
                        warn!("failed to read feed id={feed_id}: {e:#}");
                        continue;
                    }
                };

                (feed_id, feed.url)
            }
            IngestJob::Url(url) => (0, url),
        };

        info!("ingesting url={url}");
        match ingest_feed_url(&pool, &client, &url, &new_item_tx).await {
            Ok(result) => {
                counter!("rss_centr_feeds_ingested_total").increment(1);
                counter!("rss_centr_feed_items_inserted_total")
                    .increment(result.inserted_items as u64);
                info!(
                    "ingest ok url={url} feed_id={} inserted_items={}",
                    result.feed_id, result.inserted_items
                );
            }
            Err(e) => {
                counter!("rss_centr_ingest_errors_total").increment(1);
                warn!("ingest failed url={url} feed_id={feed_id}: {e:#}");
            }
        }
    }
}

pub async fn enqueue_due_feeds_loop(
    pool: SqlitePool,
    tx: Sender<IngestJob>,
    every: Duration,
) -> anyhow::Result<()> {
    loop {
        let now = Utc::now();
        let due = feed_subscription::list_due_feeds(&pool, now).await?;

        for feed in due {
            if let Err(e) = tx.send(IngestJob::FeedId(feed.id)).await {
                warn!("failed to queue ingest for feed_id={}: {e}", feed.id);
            }
        }

        tokio::time::sleep(every).await;
    }
}
