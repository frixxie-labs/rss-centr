use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use feed_rs::model::{Entry, Text};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::events::NewFeedItemEvent;

use super::feed::{FetchFeedOutcome, fetch_feed_with_cache};
use super::feed_item::{insert_feed_item_dedup, insert_feed_item_detail_dedup};
use super::feed_subscription::{touch_feed_failure, touch_feed_success, upsert_feed_by_url};

const MAX_FALLBACK_POLL_INTERVAL_SECONDS: i64 = 6_000;

pub struct IngestResult {
    pub feed_id: i64,
    pub inserted_items: usize,
}

pub async fn ingest_feed_url(
    pool: &SqlitePool,
    client: &reqwest::Client,
    url: &str,
    new_item_tx: &broadcast::Sender<NewFeedItemEvent>,
) -> Result<IngestResult> {
    let feed_sub = upsert_feed_by_url(pool, url).await?;
    let checked_at = Utc::now();

    let fetch_outcome = match fetch_feed_with_cache(
        client,
        url,
        feed_sub.etag.as_deref(),
        feed_sub.last_modified.as_deref(),
    )
    .await
    {
        Ok(f) => f,
        Err(e) => {
            touch_feed_failure(pool, feed_sub.id, checked_at).await?;
            return Err(e);
        }
    };

    let (feed, etag, last_modified) = match fetch_outcome {
        FetchFeedOutcome::NotModified {
            etag,
            last_modified,
        } => {
            touch_feed_success(
                pool,
                feed_sub.id,
                checked_at,
                None,
                None,
                etag.as_deref(),
                last_modified.as_deref(),
                None,
            )
            .await?;

            return Ok(IngestResult {
                feed_id: feed_sub.id,
                inserted_items: 0,
            });
        }
        FetchFeedOutcome::Fetched {
            feed,
            etag,
            last_modified,
        } => (feed, etag, last_modified),
    };

    let title = feed.title.as_ref().map(text_value);
    let site_url = feed.links.first().map(|l| l.href.as_str());
    let poll_interval_seconds = Some(resolved_poll_interval_seconds(&feed.entries));
    touch_feed_success(
        pool,
        feed_sub.id,
        checked_at,
        title,
        site_url,
        etag.as_deref(),
        last_modified.as_deref(),
        poll_interval_seconds,
    )
    .await?;

    let mut inserted_items = 0usize;
    for entry in &feed.entries {
        if ingest_entry(pool, feed_sub.id, entry, new_item_tx).await? {
            inserted_items += 1;
        }
    }

    Ok(IngestResult {
        feed_id: feed_sub.id,
        inserted_items,
    })
}

async fn ingest_entry(
    pool: &SqlitePool,
    feed_id: i64,
    entry: &Entry,
    new_item_tx: &broadcast::Sender<NewFeedItemEvent>,
) -> Result<bool> {
    let external_id = entry_external_id(entry);
    let title = entry.title.as_ref().map(text_value).unwrap_or("(no title)");
    let url = entry.links.first().map(|l| l.href.as_str()).unwrap_or("");

    let mut tx = pool
        .begin()
        .await
        .with_context(|| format!("failed to begin entry ingest tx for feed_id={feed_id}"))?;

    let inserted = insert_feed_item_dedup(&mut *tx, feed_id, &external_id, title, url).await?;
    let Some(item) = inserted else {
        return Ok(false);
    };

    let (summary, content) = entry_summary_and_content(entry);
    let author = entry.authors.first().map(|a| a.name.as_str()).unwrap_or("");
    let published_at = entry_published_at(entry).unwrap_or_else(Utc::now);

    let _detail =
        insert_feed_item_detail_dedup(&mut *tx, item.id, &summary, &content, author, published_at)
            .await
            .with_context(|| format!("failed to insert detail for feed_item_id={}", item.id))?;

    tx.commit().await.with_context(|| {
        format!(
            "failed to commit entry ingest transaction for feed_item_id={} external_id={}",
            item.id, external_id
        )
    })?;

    let _ = new_item_tx.send(NewFeedItemEvent::from(&item));

    Ok(true)
}

fn text_value(t: &Text) -> &str {
    t.content.as_str()
}

fn entry_external_id(entry: &Entry) -> String {
    if !entry.id.is_empty() {
        return entry.id.clone();
    }

    if let Some(link) = entry.links.first() {
        return link.href.clone();
    }

    let title = entry
        .title
        .as_ref()
        .map(text_value)
        .unwrap_or("")
        .to_string();
    let published = entry_published_at(entry)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();

    format!("fallback:{title}:{published}")
}

fn entry_summary_and_content(entry: &Entry) -> (String, String) {
    let summary = entry
        .summary
        .as_ref()
        .map(text_value)
        .unwrap_or("")
        .to_string();

    let content = entry
        .content
        .as_ref()
        .and_then(|c| c.body.as_deref())
        .unwrap_or("")
        .to_string();

    (summary, content)
}

fn entry_published_at(entry: &Entry) -> Option<DateTime<Utc>> {
    entry
        .published
        .or(entry.updated)
        .map(|dt| dt.with_timezone(&Utc))
}

fn average_update_frequency_seconds(entries: &[Entry]) -> Option<i64> {
    let mut timestamps: Vec<_> = entries.iter().filter_map(entry_published_at).collect();
    if timestamps.len() < 2 {
        return None;
    }

    timestamps.sort_unstable_by(|a, b| b.cmp(a));

    let mut sum = 0i64;
    let mut count = 0i64;

    for window in timestamps.windows(2) {
        let newer = window[0];
        let older = window[1];
        let diff = newer.signed_duration_since(older).num_seconds();
        if diff > 0 {
            sum += diff;
            count += 1;
        }
    }

    if count == 0 {
        return None;
    }

    Some(sum / count)
}

fn resolved_poll_interval_seconds(entries: &[Entry]) -> i64 {
    average_update_frequency_seconds(entries)
        .unwrap_or(MAX_FALLBACK_POLL_INTERVAL_SECONDS)
        .min(MAX_FALLBACK_POLL_INTERVAL_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn test_entry_external_id_prefers_id() {
        let e = Entry {
            id: "abc".to_string(),
            ..Default::default()
        };
        assert_eq!(entry_external_id(&e), "abc");
    }

    #[test]
    fn test_average_update_frequency_seconds() {
        let now = Utc::now();
        let entries = vec![
            Entry {
                published: Some((now - Duration::minutes(30)).into()),
                ..Default::default()
            },
            Entry {
                published: Some((now - Duration::hours(1)).into()),
                ..Default::default()
            },
            Entry {
                published: Some((now - Duration::hours(2)).into()),
                ..Default::default()
            },
        ];

        let interval = average_update_frequency_seconds(&entries);
        assert_eq!(interval, Some(2700));
    }

    #[test]
    fn test_average_update_frequency_seconds_not_enough_dates() {
        let entries = vec![Entry::default()];
        let interval = average_update_frequency_seconds(&entries);
        assert_eq!(interval, None);
    }

    #[test]
    fn test_resolved_poll_interval_seconds_falls_back_to_max() {
        let entries = vec![Entry::default()];
        let interval = resolved_poll_interval_seconds(&entries);
        assert_eq!(interval, 6000);
    }

    #[test]
    fn test_resolved_poll_interval_seconds_caps_large_average() {
        let now = Utc::now();
        let entries = vec![
            Entry {
                published: Some((now - Duration::hours(4)).into()),
                ..Default::default()
            },
            Entry {
                published: Some((now - Duration::hours(8)).into()),
                ..Default::default()
            },
        ];

        let interval = resolved_poll_interval_seconds(&entries);
        assert_eq!(interval, 6000);
    }
}
