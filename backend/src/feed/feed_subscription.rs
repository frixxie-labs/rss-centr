use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::prelude::FromRow;
use utoipa::ToSchema;

#[derive(Debug, Serialize, Deserialize, FromRow, ToSchema)]
pub struct FeedSubscription {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub poll_interval_seconds: i64,
    pub is_enabled: bool,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_inserted_at: Option<DateTime<Utc>>,
    pub failure_count: i64,
}

pub async fn upsert_feed_by_url(pool: &PgPool, url: &str) -> Result<FeedSubscription> {
    let row = sqlx::query_as!(
        FeedSubscription,
        r#"
        INSERT INTO feeds (url)
        VALUES ($1)
        ON CONFLICT(url)
        DO UPDATE SET url = excluded.url
        RETURNING id as "id!: i64",
                  url,
                  title,
                  site_url,
                  etag,
                  last_modified,
                  poll_interval_seconds,
                  is_enabled as "is_enabled!: bool",
                  last_checked_at as "last_checked_at: _",
                  last_success_at as "last_success_at: _",
                  last_inserted_at as "last_inserted_at: _",
                  failure_count
        "#,
        url,
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to upsert feed with url={url}"))?;

    Ok(row)
}

pub async fn read_feed(pool: &PgPool, id: i64) -> Result<FeedSubscription> {
    let row = sqlx::query_as!(
        FeedSubscription,
        r#"
        SELECT id as "id!: i64",
               url,
               title,
               site_url,
               etag,
               last_modified,
               poll_interval_seconds,
               is_enabled as "is_enabled!: bool",
               last_checked_at as "last_checked_at: _",
               last_success_at as "last_success_at: _",
               last_inserted_at as "last_inserted_at: _",
               failure_count
        FROM feeds
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to read feed with id={id}"))?;

    let Some(row) = row else {
        anyhow::bail!("no feed found with id={id}");
    };

    Ok(row)
}

pub async fn list_feeds(pool: &PgPool) -> Result<Vec<FeedSubscription>> {
    let rows = sqlx::query_as!(
        FeedSubscription,
        r#"
        SELECT id as "id!: i64",
               url,
               title,
               site_url,
               etag,
               last_modified,
               poll_interval_seconds,
               is_enabled as "is_enabled!: bool",
               last_checked_at as "last_checked_at: _",
               last_success_at as "last_success_at: _",
               last_inserted_at as "last_inserted_at: _",
               failure_count
        FROM feeds
        ORDER BY id ASC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to list feeds")?;

    Ok(rows)
}

pub async fn set_feed_enabled(pool: &PgPool, id: i64, is_enabled: bool) -> Result<()> {
    let result = sqlx::query!(
        r#"
        UPDATE feeds
        SET is_enabled = $1
        WHERE id = $2
        "#,
        is_enabled,
        id,
    )
    .execute(pool)
    .await
    .with_context(|| format!("failed to update feed enabled state for id={id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={id}");
    }

    Ok(())
}

/// Fields for recording a successful feed check. Constructed by callers and
/// applied inside the feed update queue's completion transaction (see
/// `feed_update_queue::touch_feed_success_in_tx`) -- there is no pool-based
/// equivalent here since every write path now goes through that queue.
pub struct FeedSuccessUpdate<'a> {
    pub checked_at: DateTime<Utc>,
    pub title: Option<&'a str>,
    pub site_url: Option<&'a str>,
    pub etag: Option<&'a str>,
    pub last_modified: Option<&'a str>,
    pub poll_interval_seconds: Option<i64>,
    pub last_inserted_at: Option<DateTime<Utc>>,
}

pub async fn delete_feed(pool: &PgPool, id: i64) -> Result<()> {
    let result = sqlx::query!(
        r#"
        DELETE FROM feeds
        WHERE id = $1
        "#,
        id,
    )
    .execute(pool)
    .await
    .with_context(|| format!("failed to delete feed with id={id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={id}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn test_upsert_feed_by_url_is_idempotent(pool: PgPool) {
        let a = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();
        let b = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        assert_eq!(a.id, b.id);
        assert_eq!(a.url, b.url);
    }

    #[sqlx::test]
    async fn test_list_and_read_feeds(pool: PgPool) {
        let f1 = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let f2 = upsert_feed_by_url(&pool, "https://example.com/b.xml")
            .await
            .unwrap();

        let feeds = list_feeds(&pool).await.unwrap();
        assert!(feeds.len() >= 2);

        let read_back = read_feed(&pool, f2.id).await.unwrap();
        assert_eq!(read_back.id, f2.id);
        assert_eq!(read_back.url, f2.url);
        assert_ne!(f1.id, f2.id);
    }

    #[sqlx::test]
    async fn test_set_feed_enabled(pool: PgPool) {
        let f = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        set_feed_enabled(&pool, f.id, false).await.unwrap();
        let r = read_feed(&pool, f.id).await.unwrap();
        assert!(!r.is_enabled);
    }
}
