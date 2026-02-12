use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use feed_rs::model::{Entry, Text};
use sqlx::SqlitePool;
use tokio::sync::broadcast;

use crate::events::NewFeedItemEvent;

use super::feed::fetch_feed;
use super::feed_item::{insert_feed_item_dedup, insert_feed_item_detail_dedup};
use super::feed_subscription::{touch_feed_failure, touch_feed_success, upsert_feed_by_url};

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

    let feed = match fetch_feed(client, url).await {
        Ok(f) => f,
        Err(e) => {
            touch_feed_failure(pool, feed_sub.id, checked_at).await?;
            return Err(e);
        }
    };

    let title = feed.title.as_ref().map(text_value);
    let site_url = feed.links.first().map(|l| l.href.as_str());
    touch_feed_success(pool, feed_sub.id, checked_at, title, site_url).await?;

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

    let inserted = insert_feed_item_dedup(pool, feed_id, &external_id, title, url).await?;
    let Some(item) = inserted else {
        return Ok(false);
    };

    let (summary, content) = entry_summary_and_content(entry);
    let author = entry.authors.first().map(|a| a.name.as_str()).unwrap_or("");
    let published_at = entry_published_at(entry).unwrap_or_else(Utc::now);

    let _detail =
        insert_feed_item_detail_dedup(pool, item.id, &summary, &content, author, published_at)
            .await
            .with_context(|| format!("failed to insert detail for feed_item_id={}", item.id))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_external_id_prefers_id() {
        let mut e = Entry::default();
        e.id = "abc".to_string();
        assert_eq!(entry_external_id(&e), "abc");
    }
}
