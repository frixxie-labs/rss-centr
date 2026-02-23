use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
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
    pub failure_count: i64,
}

pub async fn list_enabled_feeds(pool: &SqlitePool) -> Result<Vec<FeedSubscription>> {
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
               failure_count
        FROM feeds
        WHERE is_enabled = 1
        ORDER BY id ASC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to list enabled feeds")?;

    Ok(rows)
}

pub async fn list_due_feeds(
    pool: &SqlitePool,
    now: DateTime<Utc>,
) -> Result<Vec<FeedSubscription>> {
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
               failure_count
        FROM feeds
        WHERE is_enabled = 1
          AND (
              last_checked_at IS NULL
              OR (strftime('%s', $1) - strftime('%s', last_checked_at)) >= (
                  poll_interval_seconds
                  + (
                      id % (
                          MIN(30, MAX(1, poll_interval_seconds / 10)) + 1
                      )
                  )
              )
          )
        ORDER BY id ASC
        "#,
        now,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to list due feeds at {now}"))?;

    Ok(rows)
}

pub async fn upsert_feed_by_url(pool: &SqlitePool, url: &str) -> Result<FeedSubscription> {
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
                  failure_count
        "#,
        url,
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to upsert feed with url={url}"))?;

    Ok(row)
}

pub async fn read_feed(pool: &SqlitePool, id: i64) -> Result<FeedSubscription> {
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

pub async fn list_feeds(pool: &SqlitePool) -> Result<Vec<FeedSubscription>> {
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

pub async fn set_feed_enabled(pool: &SqlitePool, id: i64, is_enabled: bool) -> Result<()> {
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

pub async fn touch_feed_success(
    pool: &SqlitePool,
    id: i64,
    checked_at: DateTime<Utc>,
    title: Option<&str>,
    site_url: Option<&str>,
    etag: Option<&str>,
    last_modified: Option<&str>,
    poll_interval_seconds: Option<i64>,
) -> Result<()> {
    let result = sqlx::query!(
        r#"
        UPDATE feeds
        SET title = COALESCE($1, title),
            site_url = COALESCE($2, site_url),
            etag = COALESCE($3, etag),
            last_modified = COALESCE($4, last_modified),
            poll_interval_seconds = COALESCE($5, poll_interval_seconds),
            last_checked_at = $6,
            last_success_at = $6,
            failure_count = 0
        WHERE id = $7
        "#,
        title,
        site_url,
        etag,
        last_modified,
        poll_interval_seconds,
        checked_at,
        id,
    )
    .execute(pool)
    .await
    .with_context(|| format!("failed to update feed success for id={id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={id}");
    }

    Ok(())
}

pub async fn touch_feed_failure(
    pool: &SqlitePool,
    id: i64,
    checked_at: DateTime<Utc>,
    poll_interval_seconds: Option<i64>,
) -> Result<()> {
    let result = sqlx::query!(
        r#"
        UPDATE feeds
        SET last_checked_at = $1,
            failure_count = failure_count + 1,
            poll_interval_seconds = COALESCE($2, poll_interval_seconds)
        WHERE id = $3
        "#,
        checked_at,
        poll_interval_seconds,
        id,
    )
    .execute(pool)
    .await
    .with_context(|| format!("failed to update feed failure for id={id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={id}");
    }

    Ok(())
}

pub async fn delete_feed(pool: &SqlitePool, id: i64) -> Result<()> {
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

    fn jitter_seconds(feed_id: i64, poll_interval_seconds: i64) -> i64 {
        let jitter_cap = (poll_interval_seconds / 10).clamp(1, 30);
        feed_id % (jitter_cap + 1)
    }

    #[sqlx::test]
    async fn test_upsert_feed_by_url_is_idempotent(pool: SqlitePool) {
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
    async fn test_list_and_read_feeds(pool: SqlitePool) {
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
    async fn test_set_feed_enabled(pool: SqlitePool) {
        let f = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        set_feed_enabled(&pool, f.id, false).await.unwrap();
        let r = read_feed(&pool, f.id).await.unwrap();
        assert!(!r.is_enabled);
    }

    #[sqlx::test]
    async fn test_touch_success_and_failure(pool: SqlitePool) {
        let f = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let t1 = Utc::now();
        touch_feed_failure(&pool, f.id, t1, Some(1200))
            .await
            .unwrap();
        let r1 = read_feed(&pool, f.id).await.unwrap();
        assert_eq!(r1.failure_count, 1);
        assert_eq!(r1.last_checked_at.unwrap(), t1);
        assert_eq!(r1.poll_interval_seconds, 1200);

        let t2 = Utc::now();
        touch_feed_success(
            &pool,
            f.id,
            t2,
            Some("Example"),
            Some("https://example.com"),
            Some("\"etag-123\""),
            Some("Mon, 01 Jan 2024 00:00:00 GMT"),
            Some(900),
        )
        .await
        .unwrap();

        let r2 = read_feed(&pool, f.id).await.unwrap();
        assert_eq!(r2.failure_count, 0);
        assert_eq!(r2.last_checked_at.unwrap(), t2);
        assert_eq!(r2.last_success_at.unwrap(), t2);
        assert_eq!(r2.title.as_deref(), Some("Example"));
        assert_eq!(r2.site_url.as_deref(), Some("https://example.com"));
        assert_eq!(r2.etag.as_deref(), Some("\"etag-123\""));
        assert_eq!(r2.poll_interval_seconds, 900);
        assert_eq!(
            r2.last_modified.as_deref(),
            Some("Mon, 01 Jan 2024 00:00:00 GMT")
        );
    }

    #[sqlx::test]
    async fn test_list_due_feeds_applies_per_feed_jitter(pool: SqlitePool) {
        let f1 = upsert_feed_by_url(&pool, "https://example.com/jitter-a.xml")
            .await
            .unwrap();
        let f2 = upsert_feed_by_url(&pool, "https://example.com/jitter-b.xml")
            .await
            .unwrap();

        let checked_at = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let poll_interval = 100i64;

        sqlx::query!(
            r#"
            UPDATE feeds
            SET poll_interval_seconds = $1,
                last_checked_at = $2
            WHERE id IN ($3, $4)
            "#,
            poll_interval,
            checked_at,
            f1.id,
            f2.id,
        )
        .execute(&pool)
        .await
        .unwrap();

        let f1_jitter = jitter_seconds(f1.id, poll_interval);
        let f2_jitter = jitter_seconds(f2.id, poll_interval);

        let (early_id, early_jitter, late_id, late_jitter) = if f1_jitter <= f2_jitter {
            (f1.id, f1_jitter, f2.id, f2_jitter)
        } else {
            (f2.id, f2_jitter, f1.id, f1_jitter)
        };

        let first_due_at = checked_at + chrono::Duration::seconds(poll_interval + early_jitter);

        let due_now = list_due_feeds(&pool, first_due_at).await.unwrap();
        assert!(due_now.iter().any(|f| f.id == early_id));
        assert!(due_now.iter().all(|f| f.id != late_id));

        let second_due_at = checked_at + chrono::Duration::seconds(poll_interval + late_jitter);
        let due_later = list_due_feeds(&pool, second_due_at).await.unwrap();
        assert!(due_later.iter().any(|f| f.id == early_id));
        assert!(due_later.iter().any(|f| f.id == late_id));
    }
}
