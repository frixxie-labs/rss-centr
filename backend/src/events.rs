use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::{PgPool, postgres::PgListener};
use tokio::sync::broadcast;
use tokio::time::{Instant, MissedTickBehavior, interval_at, sleep};
use tracing::{debug, info, warn};

use crate::feed::feed_item::{FeedItem, read_feed_item, read_feed_items_after_id};

pub const NEWS_FEED_ITEMS_CHANNEL: &str = "news_feed_items";

const LISTENER_CATCH_UP_LIMIT: i64 = 1 << 12;
const LISTENER_CATCH_UP_INTERVAL: Duration = Duration::from_secs(15);
const LISTENER_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize)]
pub struct NewFeedItemEvent {
    pub id: i64,
    pub feed_id: i64,
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub inserted_at: DateTime<Utc>,
}

impl From<&FeedItem> for NewFeedItemEvent {
    fn from(item: &FeedItem) -> Self {
        Self {
            id: item.id,
            feed_id: item.feed_id,
            external_id: item.external_id.clone(),
            title: item.title.clone(),
            url: item.url.clone(),
            inserted_at: item.inserted_at,
        }
    }
}

pub struct NewFeedItemListener {
    listener: PgListener,
    last_seen_id: i64,
}

impl NewFeedItemListener {
    pub async fn connect(pool: &PgPool) -> Result<Self> {
        info!(
            channel = NEWS_FEED_ITEMS_CHANNEL,
            "starting new feed item notification listener"
        );

        let mut listener = PgListener::connect_with(pool)
            .await
            .context("failed to connect Postgres feed item listener")?;
        listener
            .listen(NEWS_FEED_ITEMS_CHANNEL)
            .await
            .with_context(|| format!("failed to listen on {NEWS_FEED_ITEMS_CHANNEL}"))?;

        let last_seen_id = current_feed_item_high_watermark(pool).await?;
        info!(
            channel = NEWS_FEED_ITEMS_CHANNEL,
            last_seen_id = last_seen_id,
            "new feed item notification listener subscribed"
        );

        Ok(Self {
            listener,
            last_seen_id,
        })
    }

    pub async fn run(
        mut self,
        pool: PgPool,
        tx: broadcast::Sender<NewFeedItemEvent>,
    ) -> Result<()> {
        let mut catch_up = interval_at(
            Instant::now() + LISTENER_CATCH_UP_INTERVAL,
            LISTENER_CATCH_UP_INTERVAL,
        );
        catch_up.set_missed_tick_behavior(MissedTickBehavior::Delay);

        info!(
            channel = NEWS_FEED_ITEMS_CHANNEL,
            last_seen_id = self.last_seen_id,
            catch_up_limit = LISTENER_CATCH_UP_LIMIT,
            catch_up_interval_seconds = LISTENER_CATCH_UP_INTERVAL.as_secs(),
            "new feed item notification listener running"
        );

        loop {
            tokio::select! {
                notification = self.listener.recv() => {
                    match notification {
                        Ok(notification) => {
                            if notification.channel() != NEWS_FEED_ITEMS_CHANNEL {
                                continue;
                            }

                            let payload = notification.payload();
                            let item_id = match payload.parse::<i64>() {
                                Ok(item_id) => item_id,
                                Err(e) => {
                                    warn!(
                                        channel = notification.channel(),
                                        process_id = notification.process_id(),
                                        payload = payload,
                                        error = %e,
                                        "ignoring invalid feed item notification payload"
                                    );
                                    continue;
                                }
                            };

                            debug!(
                                channel = notification.channel(),
                                process_id = notification.process_id(),
                                feed_item_id = item_id,
                                last_seen_id = self.last_seen_id,
                                sse_receiver_count = tx.receiver_count(),
                                "received new feed item notification"
                            );

                            if let Err(e) = broadcast_feed_item_by_id(&pool, &tx, item_id, "notification").await {
                                warn!(
                                    feed_item_id = item_id,
                                    error = %e,
                                    "failed to broadcast notified feed item"
                                );
                            } else {
                                self.last_seen_id = self.last_seen_id.max(item_id);
                                debug!(
                                    feed_item_id = item_id,
                                    last_seen_id = self.last_seen_id,
                                    "advanced new feed item listener cursor after notification"
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                channel = NEWS_FEED_ITEMS_CHANNEL,
                                retry_delay_ms = LISTENER_RETRY_DELAY.as_millis() as u64,
                                error = %e,
                                "feed item listener receive failed; retrying"
                            );
                            sleep(LISTENER_RETRY_DELAY).await;
                        }
                    }
                }
                _ = catch_up.tick() => {
                    if let Err(e) = broadcast_feed_items_after_id(
                        &pool,
                        &tx,
                        &mut self.last_seen_id,
                        LISTENER_CATCH_UP_LIMIT,
                    ).await {
                        warn!(
                            last_seen_id = self.last_seen_id,
                            limit = LISTENER_CATCH_UP_LIMIT,
                            error = %e,
                            "failed to catch up feed item notifications"
                        );
                    }
                }
            }
        }
    }
}

async fn current_feed_item_high_watermark(pool: &PgPool) -> Result<i64> {
    let max_id = sqlx::query_scalar::<_, Option<i64>>("SELECT MAX(id) FROM feed_items")
        .fetch_one(pool)
        .await
        .context("failed to read feed item high watermark")?;

    Ok(max_id.unwrap_or(0))
}

async fn broadcast_feed_item_by_id(
    pool: &PgPool,
    tx: &broadcast::Sender<NewFeedItemEvent>,
    item_id: i64,
    source: &'static str,
) -> Result<()> {
    let item = read_feed_item(pool, item_id).await?;
    broadcast_feed_item_event(tx, NewFeedItemEvent::from(&item), source);
    Ok(())
}

async fn broadcast_feed_items_after_id(
    pool: &PgPool,
    tx: &broadcast::Sender<NewFeedItemEvent>,
    last_seen_id: &mut i64,
    limit: i64,
) -> Result<()> {
    let start_last_seen_id = *last_seen_id;
    let replay = read_feed_items_after_id(pool, *last_seen_id, limit).await?;
    let item_count = replay.items.len();
    let first_item_id = replay.items.first().map(|item| item.id);
    let last_item_id = replay.items.last().map(|item| item.id);

    if replay.skipped_older {
        warn!(
            start_last_seen_id = start_last_seen_id,
            first_item_id = first_item_id,
            last_item_id = last_item_id,
            item_count = item_count,
            limit = limit,
            "feed item listener skipped older catch-up items"
        );
    }

    for item in replay.items {
        *last_seen_id = (*last_seen_id).max(item.id);
        broadcast_feed_item_event(tx, NewFeedItemEvent::from(&item), "catch_up");
    }

    if item_count > 0 || replay.skipped_older {
        info!(
            start_last_seen_id = start_last_seen_id,
            end_last_seen_id = *last_seen_id,
            first_item_id = first_item_id,
            last_item_id = last_item_id,
            item_count = item_count,
            skipped_older = replay.skipped_older,
            sse_receiver_count = tx.receiver_count(),
            "new feed item listener catch-up completed"
        );
    }

    Ok(())
}

fn broadcast_feed_item_event(
    tx: &broadcast::Sender<NewFeedItemEvent>,
    event: NewFeedItemEvent,
    source: &'static str,
) {
    let feed_item_id = event.id;
    let feed_id = event.feed_id;
    match tx.send(event) {
        Ok(sent_receiver_count) => {
            debug!(
                source = source,
                feed_item_id = feed_item_id,
                feed_id = feed_id,
                sent_receiver_count = sent_receiver_count,
                "broadcasted new feed item event to SSE clients"
            );
        }
        Err(_) => {
            debug!(
                source = source,
                feed_item_id = feed_item_id,
                feed_id = feed_id,
                "new feed item event had no active SSE receivers"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_item::insert_feed_item;
    use crate::feed::feed_subscription::upsert_feed_by_url;

    #[sqlx::test]
    async fn test_broadcast_feed_item_by_id_sends_new_item_event(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();
        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();
        let (tx, mut rx) = broadcast::channel(16);

        broadcast_feed_item_by_id(&pool, &tx, item.id, "test")
            .await
            .unwrap();

        let event = rx.recv().await.unwrap();
        assert_eq!(event.id, item.id);
        assert_eq!(event.feed_id, feed.id);
        assert_eq!(event.external_id, "ext-1");
    }

    #[sqlx::test]
    async fn test_broadcast_feed_items_after_id_advances_cursor(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();
        let first = insert_feed_item(&pool, feed.id, "ext-1", "One", "https://example.com/1")
            .await
            .unwrap();
        let second = insert_feed_item(&pool, feed.id, "ext-2", "Two", "https://example.com/2")
            .await
            .unwrap();
        let (tx, mut rx) = broadcast::channel(16);
        let mut last_seen_id = first.id;

        broadcast_feed_items_after_id(&pool, &tx, &mut last_seen_id, 16)
            .await
            .unwrap();

        let event = rx.recv().await.unwrap();
        assert_eq!(event.id, second.id);
        assert_eq!(last_seen_id, second.id);
    }
}
