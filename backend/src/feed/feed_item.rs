use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use sqlx::{PgPool, Postgres};
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, FromRow, ToSchema)]
pub struct FeedItem {
    pub id: i64,
    pub feed_id: i64,
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub inserted_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, FromRow, ToSchema)]
pub struct FeedItemWithDetail {
    pub id: i64,
    pub feed_id: i64,
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub inserted_at: DateTime<Utc>,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Serialize, Deserialize, FromRow, ToSchema)]
pub struct FeedItemDetail {
    pub id: i64,
    pub feed_item_id: i64,
    pub summary: String,
    pub content: String,
    pub author: String,
    pub published_at: DateTime<Utc>,
}

struct FeedCadenceStats {
    median_inserted_seconds: Option<f64>,
    median_published_seconds: Option<f64>,
    inserted_samples: i64,
    published_samples: i64,
}

/// Minimum number of diff samples before we consider a cadence signal reliable.
const MIN_CADENCE_SAMPLES: i64 = 2;

/// Choose the best cadence estimate from inserted-at and published-at diffs.
///
/// Published-at diffs reflect the feed's *actual* publishing rhythm and are
/// preferred whenever we have enough samples.  Inserted-at diffs are only used
/// as a fallback when published data is insufficient (e.g. the feed never
/// provides a publication date).
fn cadence_seconds(row: &FeedCadenceStats) -> Option<i64> {
    let has_published =
        row.published_samples >= MIN_CADENCE_SAMPLES && row.median_published_seconds.is_some();
    let has_inserted =
        row.inserted_samples >= MIN_CADENCE_SAMPLES && row.median_inserted_seconds.is_some();

    if has_published {
        // Published cadence is the most reliable signal — use it directly.
        Some(row.median_published_seconds.unwrap().round() as i64)
    } else if has_inserted {
        // Fallback: use inserted-at cadence (already filtered for bulk-ingest
        // noise in the SQL query via a minimum diff threshold).
        Some(row.median_inserted_seconds.unwrap().round() as i64)
    } else {
        None
    }
}

pub async fn insert_feed_item(
    pool: &PgPool,
    feed_id: i64,
    external_id: &str,
    title: &str,
    url: &str,
) -> Result<FeedItem> {
    let row = sqlx::query_as!(
        FeedItem,
        r#"
        INSERT INTO feed_items (feed_id, external_id, title, url)
        VALUES ($1, $2, $3, $4)
        RETURNING id as "id!: i64", feed_id, external_id, title, url,
                  inserted_at as "inserted_at: _"
        "#,
        feed_id,
        external_id,
        title,
        url,
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to insert feed item with external_id={external_id}"))?;

    Ok(row)
}

pub async fn insert_feed_item_dedup<'e, E>(
    executor: E,
    feed_id: i64,
    external_id: &str,
    title: &str,
    url: &str,
) -> Result<Option<FeedItem>>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        FeedItem,
        r#"
        INSERT INTO feed_items (feed_id, external_id, title, url)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT(feed_id, external_id) DO NOTHING
        RETURNING id as "id!: i64", feed_id, external_id, title, url,
                  inserted_at as "inserted_at: _"
        "#,
        feed_id,
        external_id,
        title,
        url,
    )
    .fetch_optional(executor)
    .await
    .with_context(|| {
        format!(
            "failed to insert (dedup) feed item with feed_id={feed_id} external_id={external_id}"
        )
    })?;

    Ok(row)
}

pub async fn read_feed_item(pool: &PgPool, id: i64) -> Result<FeedItem> {
    let row = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id, feed_id, external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        WHERE id = $1
        "#,
        id,
    )
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to read feed item with id={id}"))?;

    let Some(row) = row else {
        anyhow::bail!("no feed item found with id={id}");
    };

    Ok(row)
}

pub async fn read_feed_items_by_feed(pool: &PgPool, feed_id: i64) -> Result<Vec<FeedItem>> {
    let rows = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id as "id!: i64",
               feed_id as "feed_id!: i64",
               external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        WHERE feed_id = $1
        ORDER BY inserted_at DESC
        "#,
        feed_id,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read feed items for feed_id={feed_id}"))?;

    Ok(rows)
}

pub async fn read_feed_cadence_seconds(pool: &PgPool, feed_id: i64) -> Result<Option<i64>> {
    let row = sqlx::query_as!(
        FeedCadenceStats,
        r#"
        WITH inserted_diffs AS (
            SELECT EXTRACT(EPOCH FROM (
                       inserted_at - LAG(inserted_at) OVER (ORDER BY inserted_at ASC, id ASC)
                   ))::DOUBLE PRECISION AS diff_seconds
            FROM feed_items
            WHERE feed_id = $1
        ),
        published_diffs AS (
            SELECT EXTRACT(EPOCH FROM (
                       d.published_at - LAG(d.published_at) OVER (ORDER BY d.published_at ASC, d.id ASC)
                   ))::DOUBLE PRECISION AS diff_seconds
            FROM feed_items f
            JOIN feed_item_details d ON d.feed_item_id = f.id
            WHERE f.feed_id = $1
        ),
        inserted_filtered AS (
            SELECT diff_seconds, ROW_NUMBER() OVER (ORDER BY diff_seconds) AS rn,
                   COUNT(*) OVER () AS total
            FROM inserted_diffs
            WHERE diff_seconds >= 60
        ),
        published_filtered AS (
            SELECT diff_seconds, ROW_NUMBER() OVER (ORDER BY diff_seconds) AS rn,
                   COUNT(*) OVER () AS total
            FROM published_diffs
            WHERE diff_seconds >= 60
        )
        SELECT
            (SELECT AVG(diff_seconds) FROM inserted_filtered
             WHERE rn IN (total / 2, total / 2 + 1)) as "median_inserted_seconds?: f64",
            (SELECT AVG(diff_seconds) FROM published_filtered
             WHERE rn IN (total / 2, total / 2 + 1)) as "median_published_seconds?: f64",
            (SELECT COALESCE(MAX(total), 0) FROM inserted_filtered) as "inserted_samples!: i64",
            (SELECT COALESCE(MAX(total), 0) FROM published_filtered) as "published_samples!: i64"
        "#,
        feed_id,
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to read feed cadence stats for feed_id={feed_id}"))?;

    Ok(cadence_seconds(&row))
}

pub async fn read_latest_feed_items(pool: &PgPool, limit: i64) -> Result<Vec<FeedItem>> {
    let rows = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id as "id!: i64",
               feed_id as "feed_id!: i64",
               external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        ORDER BY inserted_at DESC, id DESC
        LIMIT $1
        "#,
        limit,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read latest feed items with limit={limit}"))?;

    Ok(rows)
}

pub async fn read_feed_items_after_id(pool: &PgPool, id: i64) -> Result<Vec<FeedItem>> {
    let rows = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id as "id!: i64",
               feed_id as "feed_id!: i64",
               external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        WHERE id > $1
        ORDER BY id ASC
        "#,
        id,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read feed items after id={id}"))?;

    Ok(rows)
}

pub async fn read_latest_feed_items_with_detail(
    pool: &PgPool,
    limit: Option<i64>,
    feed_id: Option<i64>,
    query: Option<&str>,
) -> Result<Vec<FeedItemWithDetail>> {
    let rows = sqlx::query_as!(
        FeedItemWithDetail,
        r#"
        SELECT f.id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               f.external_id,
               f.title,
               f.url,
               f.inserted_at as "inserted_at: _",
               d.summary,
               d.content,
               d.author,
               d.published_at as "published_at: _"
        FROM feed_items f
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        INNER JOIN feeds s ON s.id = f.feed_id
        CROSS JOIN LATERAL (SELECT websearch_to_tsquery('simple', $3) AS query) search
        WHERE ($2::BIGINT IS NULL OR f.feed_id = $2)
          AND (
              $3::TEXT IS NULL
              OR to_tsvector('simple', f.title || ' ' || f.url) @@ search.query
              OR to_tsvector(
                  'simple',
                  COALESCE(d.summary, '') || ' ' || COALESCE(d.content, '') || ' ' || COALESCE(d.author, '')
              ) @@ search.query
              OR to_tsvector('simple', COALESCE(s.title, '') || ' ' || s.url) @@ search.query
          )
        ORDER BY COALESCE(d.published_at, f.inserted_at) DESC, f.id DESC
        LIMIT $1
        "#,
        limit,
        feed_id,
        query,
    )
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!(
            "failed to read latest feed items with detail with limit={limit:?} feed_id={feed_id:?} query={query:?}"
        )
    })?;

    Ok(rows)
}

pub async fn read_recent_feed_items(pool: &PgPool) -> Result<Vec<FeedItem>> {
    let rows = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id as "id!: i64",
               feed_id as "feed_id!: i64",
               external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        WHERE inserted_at >= NOW() - INTERVAL '24 hours'
        ORDER BY inserted_at DESC, id DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to read recent feed items")?;

    Ok(rows)
}

pub async fn read_all_feed_items(pool: &PgPool) -> Result<Vec<FeedItem>> {
    let rows = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id as "id!: i64",
               feed_id as "feed_id!: i64",
               external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        ORDER BY inserted_at DESC, id DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to read all feed items")?;

    Ok(rows)
}

pub async fn read_all_feed_items_with_detail(pool: &PgPool) -> Result<Vec<FeedItemWithDetail>> {
    let rows = sqlx::query_as!(
        FeedItemWithDetail,
        r#"
        SELECT f.id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               f.external_id,
               f.title,
               f.url,
               f.inserted_at as "inserted_at: _",
               d.summary,
               d.content,
               d.author,
               d.published_at as "published_at: _"
        FROM feed_items f
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        ORDER BY COALESCE(d.published_at, f.inserted_at) DESC, f.id DESC
        "#,
    )
    .fetch_all(pool)
    .await
    .context("failed to read all feed items with detail")?;

    Ok(rows)
}

pub async fn update_feed_item(pool: &PgPool, id: i64, title: &str, url: &str) -> Result<FeedItem> {
    let row = sqlx::query_as!(
        FeedItem,
        r#"
        UPDATE feed_items
        SET title = $1, url = $2
        WHERE id = $3
        RETURNING id as "id!: i64", feed_id, external_id, title, url,
                  inserted_at as "inserted_at: _"
        "#,
        title,
        url,
        id,
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to update feed item with id={id}"))?;

    Ok(row)
}

pub async fn delete_feed_item(pool: &PgPool, id: i64) -> Result<()> {
    let result = sqlx::query!(
        r#"
        DELETE FROM feed_items
        WHERE id = $1
        "#,
        id,
    )
    .execute(pool)
    .await
    .with_context(|| format!("failed to delete feed item with id={id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed item found with id={id}");
    }

    Ok(())
}

pub async fn insert_feed_item_detail(
    pool: &PgPool,
    feed_item_id: i64,
    summary: &str,
    content: &str,
    author: &str,
    published_at: DateTime<Utc>,
) -> Result<FeedItemDetail> {
    let row = sqlx::query_as!(
        FeedItemDetail,
        r#"
        INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id as "id!: i64", feed_item_id as "feed_item_id!: i64",
                  summary, content, author,
                  published_at as "published_at: _"
        "#,
        feed_item_id,
        summary,
        content,
        author,
        published_at,
    )
    .fetch_one(pool)
    .await
    .with_context(|| {
        format!("failed to insert feed item detail for feed_item_id={feed_item_id}")
    })?;

    Ok(row)
}

pub async fn insert_feed_item_detail_dedup<'e, E>(
    executor: E,
    feed_item_id: i64,
    summary: &str,
    content: &str,
    author: &str,
    published_at: DateTime<Utc>,
) -> Result<Option<FeedItemDetail>>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let row = sqlx::query_as!(
        FeedItemDetail,
        r#"
        INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
        VALUES ($1, $2, $3, $4, $5)
        ON CONFLICT(feed_item_id) DO NOTHING
        RETURNING id as "id!: i64", feed_item_id as "feed_item_id!: i64",
                  summary, content, author,
                  published_at as "published_at: _"
        "#,
        feed_item_id,
        summary,
        content,
        author,
        published_at,
    )
    .fetch_optional(executor)
    .await
    .with_context(|| {
        format!("failed to insert (dedup) feed item detail for feed_item_id={feed_item_id}")
    })?;

    Ok(row)
}

pub async fn read_feed_item_detail(pool: &PgPool, feed_item_id: i64) -> Result<FeedItemDetail> {
    let row = sqlx::query_as!(
        FeedItemDetail,
        r#"
        SELECT id as "id!: i64", feed_item_id as "feed_item_id!: i64",
               summary, content, author,
               published_at as "published_at: _"
        FROM feed_item_details
        WHERE feed_item_id = $1
        "#,
        feed_item_id,
    )
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to read feed item detail for feed_item_id={feed_item_id}"))?;

    let Some(row) = row else {
        anyhow::bail!("no feed item detail found for feed_item_id={feed_item_id}");
    };

    Ok(row)
}

pub async fn update_feed_item_detail(
    pool: &PgPool,
    feed_item_id: i64,
    summary: &str,
    content: &str,
    author: &str,
    published_at: DateTime<Utc>,
) -> Result<FeedItemDetail> {
    let row = sqlx::query_as!(
        FeedItemDetail,
        r#"
        UPDATE feed_item_details
        SET summary = $1, content = $2, author = $3, published_at = $4
        WHERE feed_item_id = $5
        RETURNING id as "id!: i64", feed_item_id as "feed_item_id!: i64",
                  summary, content, author,
                  published_at as "published_at: _"
        "#,
        summary,
        content,
        author,
        published_at,
        feed_item_id,
    )
    .fetch_one(pool)
    .await
    .with_context(|| {
        format!("failed to update feed item detail for feed_item_id={feed_item_id}")
    })?;

    Ok(row)
}

pub async fn delete_feed_item_detail(pool: &PgPool, feed_item_id: i64) -> Result<()> {
    let result = sqlx::query!(
        r#"
        DELETE FROM feed_item_details
        WHERE feed_item_id = $1
        "#,
        feed_item_id,
    )
    .execute(pool)
    .await
    .with_context(|| {
        format!("failed to delete feed item detail for feed_item_id={feed_item_id}")
    })?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed item detail found for feed_item_id={feed_item_id}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_subscription::upsert_feed_by_url;

    #[test]
    fn test_cadence_seconds_prefers_published_when_sufficient() {
        let row = FeedCadenceStats {
            median_inserted_seconds: Some(3_600.0),
            median_published_seconds: Some(300.0),
            inserted_samples: 2,
            published_samples: 18,
        };

        // published_samples >= MIN_CADENCE_SAMPLES, so published median wins
        let cadence = cadence_seconds(&row);
        assert_eq!(cadence, Some(300));
    }

    #[test]
    fn test_cadence_seconds_falls_back_to_inserted() {
        let row = FeedCadenceStats {
            median_inserted_seconds: Some(3_600.0),
            median_published_seconds: None,
            inserted_samples: 5,
            published_samples: 0,
        };

        let cadence = cadence_seconds(&row);
        assert_eq!(cadence, Some(3_600));
    }

    #[test]
    fn test_cadence_seconds_ignores_insufficient_samples() {
        let row = FeedCadenceStats {
            median_inserted_seconds: Some(100.0),
            median_published_seconds: Some(200.0),
            inserted_samples: 1,  // below MIN_CADENCE_SAMPLES
            published_samples: 1, // below MIN_CADENCE_SAMPLES
        };

        let cadence = cadence_seconds(&row);
        assert_eq!(cadence, None);
    }

    #[test]
    fn test_cadence_seconds_handles_empty_samples() {
        let row = FeedCadenceStats {
            median_inserted_seconds: None,
            median_published_seconds: None,
            inserted_samples: 0,
            published_samples: 0,
        };

        let cadence = cadence_seconds(&row);
        assert_eq!(cadence, None);
    }

    // -----------------------------------------------------------------------
    // FeedItem tests
    // -----------------------------------------------------------------------

    #[sqlx::test]
    async fn test_read_feed_cadence_seconds_prefers_published_over_inserted(pool: PgPool) {
        // Scenario: feed was just subscribed to, so inserted_at values are
        // close together (bulk ingest), but published_at values reflect the
        // real publishing cadence.
        let feed = upsert_feed_by_url(&pool, "https://example.com/cadence.xml")
            .await
            .unwrap();

        let item1 = insert_feed_item(&pool, feed.id, "cadence-1", "One", "https://example.com/1")
            .await
            .unwrap();
        let item2 = insert_feed_item(&pool, feed.id, "cadence-2", "Two", "https://example.com/2")
            .await
            .unwrap();
        let item3 = insert_feed_item(
            &pool,
            feed.id,
            "cadence-3",
            "Three",
            "https://example.com/3",
        )
        .await
        .unwrap();

        let now = Utc::now();
        insert_feed_item_detail(&pool, item1.id, "", "", "", now)
            .await
            .unwrap();
        insert_feed_item_detail(&pool, item2.id, "", "", "", now)
            .await
            .unwrap();
        insert_feed_item_detail(&pool, item3.id, "", "", "", now)
            .await
            .unwrap();

        // Bulk-ingested: all inserted_at values within 2 seconds (< 60s threshold).
        let inserted_1 = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let inserted_2 = DateTime::parse_from_rfc3339("2024-01-01T00:00:01Z")
            .unwrap()
            .with_timezone(&Utc);
        let inserted_3 = DateTime::parse_from_rfc3339("2024-01-01T00:00:02Z")
            .unwrap()
            .with_timezone(&Utc);

        // Real published_at cadence: 2 hours apart.
        let published_1 = DateTime::parse_from_rfc3339("2023-12-31T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let published_2 = DateTime::parse_from_rfc3339("2023-12-31T22:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let published_3 = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(inserted_1)
            .bind(item1.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(inserted_2)
            .bind(item2.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(inserted_3)
            .bind(item3.id)
            .execute(&pool)
            .await
            .unwrap();

        sqlx::query(r#"UPDATE feed_item_details SET published_at = $1 WHERE feed_item_id = $2"#)
            .bind(published_1)
            .bind(item1.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_item_details SET published_at = $1 WHERE feed_item_id = $2"#)
            .bind(published_2)
            .bind(item2.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_item_details SET published_at = $1 WHERE feed_item_id = $2"#)
            .bind(published_3)
            .bind(item3.id)
            .execute(&pool)
            .await
            .unwrap();

        let cadence = read_feed_cadence_seconds(&pool, feed.id).await.unwrap();
        // inserted_diffs: 1s, 1s — both < 60s, so 0 qualifying samples.
        // published_diffs: 7200s, 7200s — both >= 60s, so avg = 7200.
        // Published data is preferred and sufficient (2 samples), so cadence = 7200.
        assert_eq!(cadence, Some(7200));
    }

    #[sqlx::test]
    async fn test_read_feed_cadence_seconds_falls_back_to_inserted_diffs(pool: PgPool) {
        // Scenario: feed items have no real published_at (all inferred to
        // insertion time), so published_diffs are all < 60s. Falls back to
        // inserted_diffs which have a real cadence.
        let feed = upsert_feed_by_url(&pool, "https://example.com/cadence-fallback.xml")
            .await
            .unwrap();

        let item1 = insert_feed_item(&pool, feed.id, "fb-1", "One", "https://example.com/fb/1")
            .await
            .unwrap();
        let item2 = insert_feed_item(&pool, feed.id, "fb-2", "Two", "https://example.com/fb/2")
            .await
            .unwrap();
        let item3 = insert_feed_item(&pool, feed.id, "fb-3", "Three", "https://example.com/fb/3")
            .await
            .unwrap();

        // Inserted at 10-minute intervals (real polling cadence).
        let inserted_1 = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let inserted_2 = DateTime::parse_from_rfc3339("2024-01-01T00:10:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let inserted_3 = DateTime::parse_from_rfc3339("2024-01-01T00:20:00Z")
            .unwrap()
            .with_timezone(&Utc);

        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(inserted_1)
            .bind(item1.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(inserted_2)
            .bind(item2.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(inserted_3)
            .bind(item3.id)
            .execute(&pool)
            .await
            .unwrap();

        // Published_at = inserted_at (inferred, no real publication date).
        // This means published_diffs = inserted_diffs = 600s, which IS >= 60s.
        // Both signals have 2 samples, but published is preferred.
        insert_feed_item_detail(&pool, item1.id, "", "", "", inserted_1)
            .await
            .unwrap();
        insert_feed_item_detail(&pool, item2.id, "", "", "", inserted_2)
            .await
            .unwrap();
        insert_feed_item_detail(&pool, item3.id, "", "", "", inserted_3)
            .await
            .unwrap();

        let cadence = read_feed_cadence_seconds(&pool, feed.id).await.unwrap();
        // Both inserted_diffs and published_diffs average 600s.
        // Published is preferred: cadence = 600.
        assert_eq!(cadence, Some(600));
    }

    #[sqlx::test]
    async fn test_insert_feed_item(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        assert_eq!(item.feed_id, feed.id);
        assert_eq!(item.external_id, "ext-1");
        assert_eq!(item.title, "Title");
        assert_eq!(item.url, "https://example.com");
    }

    #[sqlx::test]
    async fn test_insert_feed_item_dedup(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let a = insert_feed_item_dedup(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();
        assert!(a.is_some());

        let b = insert_feed_item_dedup(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();
        assert!(b.is_none());
    }

    #[sqlx::test]
    async fn test_read_feed_item(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let inserted = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let fetched = read_feed_item(&pool, inserted.id).await.unwrap();

        assert_eq!(fetched.id, inserted.id);
        assert_eq!(fetched.external_id, "ext-1");
        assert_eq!(fetched.title, "Title");
    }

    #[sqlx::test]
    async fn test_read_feed_item_not_found(pool: PgPool) {
        let result = read_feed_item(&pool, 999).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_read_feed_items_by_feed(pool: PgPool) {
        let feed1 = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let feed2 = upsert_feed_by_url(&pool, "https://example.com/b.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed1.id, "ext-1", "First", "https://example.com/1")
            .await
            .unwrap();
        insert_feed_item(&pool, feed1.id, "ext-2", "Second", "https://example.com/2")
            .await
            .unwrap();
        insert_feed_item(
            &pool,
            feed2.id,
            "ext-3",
            "Other feed",
            "https://example.com/3",
        )
        .await
        .unwrap();

        let items = read_feed_items_by_feed(&pool, feed1.id).await.unwrap();
        assert_eq!(items.len(), 2);

        let items_feed2 = read_feed_items_by_feed(&pool, feed2.id).await.unwrap();
        assert_eq!(items_feed2.len(), 1);
        assert_eq!(items_feed2[0].title, "Other feed");
    }

    #[sqlx::test]
    async fn test_read_feed_items_by_feed_empty(pool: PgPool) {
        let items = read_feed_items_by_feed(&pool, 42).await.unwrap();
        assert!(items.is_empty());
    }

    #[sqlx::test]
    async fn test_read_latest_feed_items_limit_and_order(pool: PgPool) {
        let feed1 = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let feed2 = upsert_feed_by_url(&pool, "https://example.com/b.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed1.id, "ext-1", "First", "https://example.com/1")
            .await
            .unwrap();
        insert_feed_item(&pool, feed2.id, "ext-2", "Second", "https://example.com/2")
            .await
            .unwrap();
        insert_feed_item(&pool, feed1.id, "ext-3", "Third", "https://example.com/3")
            .await
            .unwrap();

        let items = read_latest_feed_items(&pool, 2).await.unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Third");
        assert_eq!(items[1].title, "Second");
    }

    #[sqlx::test]
    async fn test_read_latest_feed_items_with_detail_filters(pool: PgPool) {
        let feed1 = upsert_feed_by_url(&pool, "https://example.com/rust.xml")
            .await
            .unwrap();
        let feed2 = upsert_feed_by_url(&pool, "https://example.com/db.xml")
            .await
            .unwrap();
        let now = Utc::now();

        let rust_item = insert_feed_item(
            &pool,
            feed1.id,
            "ext-1",
            "Rust release",
            "https://example.com/1",
        )
        .await
        .unwrap();
        insert_feed_item_detail(
            &pool,
            rust_item.id,
            "Compiler improvements",
            "",
            "Ferris",
            now,
        )
        .await
        .unwrap();

        let db_item = insert_feed_item(
            &pool,
            feed2.id,
            "ext-2",
            "Postgres news",
            "https://example.com/2",
        )
        .await
        .unwrap();
        insert_feed_item_detail(&pool, db_item.id, "Index tuning", "", "Pg", now)
            .await
            .unwrap();

        let by_feed = read_latest_feed_items_with_detail(&pool, Some(10), Some(feed1.id), None)
            .await
            .unwrap();
        assert_eq!(by_feed.len(), 1);
        assert_eq!(by_feed[0].title, "Rust release");

        let by_query = read_latest_feed_items_with_detail(&pool, Some(10), None, Some("index"))
            .await
            .unwrap();
        assert_eq!(by_query.len(), 1);
        assert_eq!(by_query[0].title, "Postgres news");
    }

    #[sqlx::test]
    async fn test_update_feed_item(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Old title", "https://old.com")
            .await
            .unwrap();

        let updated = update_feed_item(&pool, item.id, "New title", "https://new.com")
            .await
            .unwrap();

        assert_eq!(updated.id, item.id);
        assert_eq!(updated.title, "New title");
        assert_eq!(updated.url, "https://new.com");
        assert_eq!(updated.external_id, "ext-1"); // unchanged
    }

    #[sqlx::test]
    async fn test_delete_feed_item(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        delete_feed_item(&pool, item.id).await.unwrap();

        let result = read_feed_item(&pool, item.id).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_delete_feed_item_not_found(pool: PgPool) {
        let result = delete_feed_item(&pool, 999).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // FeedItemDetail tests
    // -----------------------------------------------------------------------

    #[sqlx::test]
    async fn test_insert_feed_item_detail(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let now = Utc::now();
        let detail = insert_feed_item_detail(&pool, item.id, "summary", "content", "author", now)
            .await
            .unwrap();

        assert_eq!(detail.feed_item_id, item.id);
        assert_eq!(detail.summary, "summary");
        assert_eq!(detail.content, "content");
        assert_eq!(detail.author, "author");
    }

    #[sqlx::test]
    async fn test_insert_feed_item_detail_dedup(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let now = Utc::now();
        let a = insert_feed_item_detail_dedup(&pool, item.id, "s", "c", "a", now)
            .await
            .unwrap();
        assert!(a.is_some());

        let b = insert_feed_item_detail_dedup(&pool, item.id, "s", "c", "a", now)
            .await
            .unwrap();
        assert!(b.is_none());
    }

    #[sqlx::test]
    async fn test_read_feed_item_detail(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let now = Utc::now();
        insert_feed_item_detail(&pool, item.id, "summary", "content", "author", now)
            .await
            .unwrap();

        let detail = read_feed_item_detail(&pool, item.id).await.unwrap();
        assert_eq!(detail.feed_item_id, item.id);
        assert_eq!(detail.summary, "summary");
    }

    #[sqlx::test]
    async fn test_read_feed_item_detail_not_found(pool: PgPool) {
        let result = read_feed_item_detail(&pool, 999).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_update_feed_item_detail(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let now = Utc::now();
        insert_feed_item_detail(
            &pool,
            item.id,
            "old summary",
            "old content",
            "old author",
            now,
        )
        .await
        .unwrap();

        let later = Utc::now();
        let updated = update_feed_item_detail(
            &pool,
            item.id,
            "new summary",
            "new content",
            "new author",
            later,
        )
        .await
        .unwrap();

        assert_eq!(updated.feed_item_id, item.id);
        assert_eq!(updated.summary, "new summary");
        assert_eq!(updated.content, "new content");
        assert_eq!(updated.author, "new author");
    }

    #[sqlx::test]
    async fn test_delete_feed_item_detail(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let now = Utc::now();
        insert_feed_item_detail(&pool, item.id, "summary", "content", "author", now)
            .await
            .unwrap();

        delete_feed_item_detail(&pool, item.id).await.unwrap();

        let result = read_feed_item_detail(&pool, item.id).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_delete_feed_item_detail_not_found(pool: PgPool) {
        let result = delete_feed_item_detail(&pool, 999).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_cascade_delete_removes_detail(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let now = Utc::now();
        insert_feed_item_detail(&pool, item.id, "summary", "content", "author", now)
            .await
            .unwrap();

        // Deleting the parent feed_item should cascade to feed_item_details
        delete_feed_item(&pool, item.id).await.unwrap();

        let result = read_feed_item_detail(&pool, item.id).await;
        assert!(result.is_err());
    }
}
