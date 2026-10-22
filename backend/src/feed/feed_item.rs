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

pub struct FeedItemsAfterId {
    pub items: Vec<FeedItem>,
    pub skipped_older: bool,
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

pub async fn read_feed_cadence_seconds<'e, E>(executor: E, feed_id: i64) -> Result<Option<i64>>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
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
    .fetch_one(executor)
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

pub async fn read_feed_items_after_id(
    pool: &PgPool,
    id: i64,
    limit: i64,
) -> Result<FeedItemsAfterId> {
    if limit <= 0 {
        return Ok(FeedItemsAfterId {
            items: Vec::new(),
            skipped_older: false,
        });
    }

    let mut rows = sqlx::query_as!(
        FeedItem,
        r#"
        SELECT id as "id!: i64",
               feed_id as "feed_id!: i64",
               external_id, title, url,
               inserted_at as "inserted_at: _"
        FROM feed_items
        WHERE id > $1
        ORDER BY id DESC
        LIMIT $2
        "#,
        id,
        limit.saturating_add(1),
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read feed items after id={id} with limit={limit}"))?;

    let skipped_older = rows.len() as i64 > limit;
    if skipped_older {
        rows.truncate(limit as usize);
    }
    rows.reverse();

    Ok(FeedItemsAfterId {
        items: rows,
        skipped_older,
    })
}

pub async fn read_latest_feed_items_with_detail(
    pool: &PgPool,
    limit: Option<i64>,
    feed_id: Option<i64>,
    query: Option<&str>,
    offset: Option<i64>,
) -> Result<Vec<FeedItemWithDetail>> {
    let rows = sqlx::query_as!(
        FeedItemWithDetail,
        r#"
        WITH search AS (SELECT websearch_to_tsquery('simple', $3) AS query),
        matches AS (
            SELECT id FROM feed_items, search WHERE search_vector @@ search.query
            UNION
            SELECT feed_item_id FROM feed_item_details, search WHERE search_vector @@ search.query
            UNION
            SELECT i.id
            FROM feeds s
            INNER JOIN feed_items i ON i.feed_id = s.id
            CROSS JOIN search
            WHERE s.search_vector @@ search.query
        )
        SELECT f.id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               f.external_id,
               f.title,
               f.url,
               f.inserted_at as "inserted_at: _",
               d.summary as "summary?",
               d.content as "content?",
               d.author as "author?",
               d.published_at as "published_at?: _"
        FROM feed_items f
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        WHERE ($2::BIGINT IS NULL OR f.feed_id = $2)
          AND ($3::TEXT IS NULL OR f.id IN (SELECT id FROM matches))
        ORDER BY LEAST(COALESCE(d.published_at, f.inserted_at), f.inserted_at) DESC, f.id DESC
        LIMIT $1
        OFFSET COALESCE($4::BIGINT, 0)
        "#,
        limit,
        feed_id,
        query,
        offset,
    )
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!(
            "failed to read latest feed items with detail with limit={limit:?} offset={offset:?} feed_id={feed_id:?} query={query:?}"
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
        ORDER BY LEAST(COALESCE(d.published_at, f.inserted_at), f.inserted_at) DESC, f.id DESC
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

/// Insert new items for `feed_id` in one statement, in the given order,
/// skipping ones that already exist. Returns `(id, external_id)` of the
/// inserted rows. `external_ids` must not contain duplicates.
pub async fn insert_feed_items_dedup_batch<'e, E>(
    executor: E,
    feed_id: i64,
    external_ids: &[String],
    titles: &[String],
    urls: &[String],
) -> Result<Vec<(i64, String)>>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    let rows = sqlx::query!(
        r#"
        INSERT INTO feed_items (feed_id, external_id, title, url)
        SELECT $1, new.external_id, new.title, new.url
        FROM UNNEST($2::TEXT[], $3::TEXT[], $4::TEXT[])
             WITH ORDINALITY AS new(external_id, title, url, ord)
        ORDER BY new.ord
        ON CONFLICT(feed_id, external_id) DO NOTHING
        RETURNING id as "id!: i64", external_id
        "#,
        feed_id,
        external_ids,
        titles,
        urls,
    )
    .fetch_all(executor)
    .await
    .with_context(|| {
        format!(
            "failed to insert {} feed items for feed_id={feed_id}",
            external_ids.len()
        )
    })?;

    Ok(rows
        .into_iter()
        .map(|row| (row.id, row.external_id))
        .collect())
}

/// Insert details for many items in one statement, skipping existing ones.
pub async fn insert_feed_item_details_dedup_batch<'e, E>(
    executor: E,
    feed_item_ids: &[i64],
    summaries: &[String],
    contents: &[String],
    authors: &[String],
    published_ats: &[DateTime<Utc>],
) -> Result<()>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    sqlx::query!(
        r#"
        INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
        SELECT *
        FROM UNNEST($1::BIGINT[], $2::TEXT[], $3::TEXT[], $4::TEXT[], $5::TIMESTAMPTZ[])
        ON CONFLICT(feed_item_id) DO NOTHING
        "#,
        feed_item_ids,
        summaries,
        contents,
        authors,
        published_ats,
    )
    .execute(executor)
    .await
    .with_context(|| {
        format!(
            "failed to insert details for {} feed items",
            feed_item_ids.len()
        )
    })?;

    Ok(())
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
    use crate::events::NEWS_FEED_ITEMS_CHANNEL;
    use crate::feed::feed_subscription::upsert_feed_by_url;
    use sqlx::postgres::PgListener;
    use tokio::time::{Duration, timeout};

    quickcheck::quickcheck! {
        fn prop_cadence_prefers_published(a: i32, b: i32, c: u8, d: u8) -> bool {
            let row = FeedCadenceStats {
                median_inserted_seconds: Some(f64::from(a)),
                median_published_seconds: Some(f64::from(b)),
                inserted_samples: i64::from(c),
                published_samples: MIN_CADENCE_SAMPLES + i64::from(d),
            };

            cadence_seconds(&row) == Some(i64::from(b))
        }

        fn prop_cadence_falls_back_inserted(a: i32, b: u8, c: u8) -> bool {
            let row = FeedCadenceStats {
                median_inserted_seconds: Some(f64::from(a)),
                median_published_seconds: None,
                inserted_samples: MIN_CADENCE_SAMPLES + i64::from(b),
                published_samples: i64::from(c),
            };

            cadence_seconds(&row) == Some(i64::from(a))
        }

        fn prop_cadence_ignores_few_samples(a: i32, b: i32, c: bool, d: bool) -> bool {
            let row = FeedCadenceStats {
                median_inserted_seconds: Some(f64::from(a)),
                median_published_seconds: Some(f64::from(b)),
                inserted_samples: if c { 1 } else { 0 },
                published_samples: if d { 1 } else { 0 },
            };

            cadence_seconds(&row).is_none()
        }

        fn prop_cadence_requires_median(a: u8, b: u8) -> bool {
            let row = FeedCadenceStats {
                median_inserted_seconds: None,
                median_published_seconds: None,
                inserted_samples: MIN_CADENCE_SAMPLES + i64::from(a),
                published_samples: MIN_CADENCE_SAMPLES + i64::from(b),
            };

            cadence_seconds(&row).is_none()
        }
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
    async fn test_insert_feed_item_notifies_news_feed_items(pool: PgPool) {
        let mut listener = PgListener::connect_with(&pool).await.unwrap();
        listener.listen(NEWS_FEED_ITEMS_CHANNEL).await.unwrap();

        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();
        let item = insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let notification = timeout(Duration::from_secs(2), listener.recv())
            .await
            .unwrap()
            .unwrap();

        assert_eq!(notification.channel(), NEWS_FEED_ITEMS_CHANNEL);
        assert_eq!(notification.payload(), item.id.to_string());
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
    async fn test_read_feed_items_after_id_limits_to_latest_missed_window(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let cursor_item = insert_feed_item(&pool, feed.id, "ext-1", "One", "https://example.com/1")
            .await
            .unwrap();
        let mut inserted = Vec::new();
        for id in 2..=6 {
            inserted.push(
                insert_feed_item(
                    &pool,
                    feed.id,
                    &format!("ext-{id}"),
                    &format!("Item {id}"),
                    &format!("https://example.com/{id}"),
                )
                .await
                .unwrap(),
            );
        }

        let replay = read_feed_items_after_id(&pool, cursor_item.id, 2)
            .await
            .unwrap();
        let replayed_ids: Vec<i64> = replay.items.iter().map(|item| item.id).collect();

        assert!(replay.skipped_older);
        assert_eq!(replayed_ids, vec![inserted[3].id, inserted[4].id]);
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

        let by_feed =
            read_latest_feed_items_with_detail(&pool, Some(10), Some(feed1.id), None, None)
                .await
                .unwrap();
        assert_eq!(by_feed.len(), 1);
        assert_eq!(by_feed[0].title, "Rust release");

        let by_query =
            read_latest_feed_items_with_detail(&pool, Some(10), None, Some("index"), None)
                .await
                .unwrap();
        assert_eq!(by_query.len(), 1);
        assert_eq!(by_query[0].title, "Postgres news");
    }

    #[sqlx::test]
    async fn test_read_latest_feed_items_with_detail_pages_with_offset(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/paged.xml")
            .await
            .unwrap();
        for n in 1..=5 {
            insert_feed_item(
                &pool,
                feed.id,
                &format!("ext-{n}"),
                &format!("Item {n}"),
                &format!("https://example.com/{n}"),
            )
            .await
            .unwrap();
        }

        let all = read_latest_feed_items_with_detail(&pool, Some(10), None, None, None)
            .await
            .unwrap();
        let first = read_latest_feed_items_with_detail(&pool, Some(2), None, None, Some(0))
            .await
            .unwrap();
        let second = read_latest_feed_items_with_detail(&pool, Some(2), None, None, Some(2))
            .await
            .unwrap();
        let last = read_latest_feed_items_with_detail(&pool, Some(2), None, None, Some(4))
            .await
            .unwrap();

        let ids = |items: &[FeedItemWithDetail]| items.iter().map(|i| i.id).collect::<Vec<_>>();
        assert_eq!([ids(&first), ids(&second), ids(&last)].concat(), ids(&all));
        assert_eq!(last.len(), 1);
    }

    #[sqlx::test]
    async fn test_read_latest_feed_items_with_detail_clamps_future_published_at(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/future.xml")
            .await
            .unwrap();

        let old_inserted = DateTime::parse_from_rfc3339("2024-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let newer_inserted = DateTime::parse_from_rfc3339("2024-01-02T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let future_published = DateTime::parse_from_rfc3339("2030-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let future_item = insert_feed_item(
            &pool,
            feed.id,
            "future",
            "Future dated",
            "https://example.com/future",
        )
        .await
        .unwrap();
        let newer_item = insert_feed_item(
            &pool,
            feed.id,
            "newer",
            "Newer inserted",
            "https://example.com/newer",
        )
        .await
        .unwrap();

        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(old_inserted)
            .bind(future_item.id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(r#"UPDATE feed_items SET inserted_at = $1 WHERE id = $2"#)
            .bind(newer_inserted)
            .bind(newer_item.id)
            .execute(&pool)
            .await
            .unwrap();

        insert_feed_item_detail(&pool, future_item.id, "", "", "", future_published)
            .await
            .unwrap();
        insert_feed_item_detail(&pool, newer_item.id, "", "", "", newer_inserted)
            .await
            .unwrap();

        let items = read_latest_feed_items_with_detail(&pool, Some(10), None, None, None)
            .await
            .unwrap();

        assert_eq!(items[0].title, "Newer inserted");
        assert_eq!(items[1].title, "Future dated");
        assert_eq!(items[1].published_at, Some(future_published));
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

    #[sqlx::test]
    async fn test_keyword_search_matches_every_searchable_field(pool: PgPool) {
        sqlx::query!("DELETE FROM feed_items")
            .execute(&pool)
            .await
            .unwrap();
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();
        let other = upsert_feed_by_url(&pool, "https://zebras.example.org/rss")
            .await
            .unwrap();
        sqlx::query!(
            "UPDATE feeds SET title = 'Giraffe Weekly' WHERE id = $1",
            feed.id
        )
        .execute(&pool)
        .await
        .unwrap();

        let now = Utc::now();
        let add = async |feed_id: i64,
                         key: &str,
                         title: &str,
                         url: &str,
                         summary: &str,
                         content: &str,
                         author: &str| {
            let item = insert_feed_item(&pool, feed_id, key, title, url)
                .await
                .unwrap();
            insert_feed_item_detail(&pool, item.id, summary, content, author, now)
                .await
                .unwrap();
            item.id
        };
        let by_title = add(other.id, "t", "Pelican sighting", "https://x/1", "", "", "").await;
        let by_summary = add(
            other.id,
            "s",
            "Untitled",
            "https://x/3",
            "<p>A walrus appears</p>",
            "",
            "",
        )
        .await;
        let by_content = add(
            other.id,
            "c",
            "Untitled",
            "https://x/4",
            "",
            "Long text about otters",
            "",
        )
        .await;
        let by_author = add(
            other.id,
            "a",
            "Untitled",
            "https://x/5",
            "",
            "",
            "Jane Narwhal",
        )
        .await;
        let by_feed_title = add(feed.id, "f", "Untitled", "https://x/6", "", "", "").await;
        let no_detail = insert_feed_item(&pool, other.id, "n", "Lonely heron", "https://x/7")
            .await
            .unwrap()
            .id;

        let search = async |q: &str| -> Vec<i64> {
            read_latest_feed_items_with_detail(&pool, Some(50), None, Some(q), None)
                .await
                .unwrap()
                .into_iter()
                .map(|i| i.id)
                .collect()
        };
        assert_eq!(search("pelican").await, vec![by_title]);
        assert_eq!(search("walrus").await, vec![by_summary]);
        assert_eq!(search("otters").await, vec![by_content]);
        assert_eq!(search("narwhal").await, vec![by_author]);
        assert_eq!(search("giraffe").await, vec![by_feed_title]);
        assert_eq!(search("heron").await, vec![no_detail]);
        assert!(
            search("pelican walrus").await.is_empty(),
            "all words must match"
        );
        assert!(search("unicorn").await.is_empty());
    }

    #[sqlx::test]
    async fn test_keyword_search_uses_precomputed_indexed_vectors(pool: PgPool) {
        // Rebuilding tsvectors for every row on each search was the main cost
        // of keyword and hybrid search; they must be stored and GIN indexed.
        for table in ["feed_items", "feed_item_details", "feeds"] {
            let indexed = sqlx::query_scalar!(
                r#"
                SELECT EXISTS (
                    SELECT 1
                    FROM pg_index i
                    JOIN pg_class t ON t.oid = i.indrelid
                    JOIN pg_class ix ON ix.oid = i.indexrelid
                    JOIN pg_am am ON am.oid = ix.relam
                    JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(i.indkey)
                    WHERE t.relname = $1 AND a.attname = 'search_vector' AND am.amname = 'gin'
                ) as "indexed!"
                "#,
                table
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            assert!(
                indexed,
                "{table}.search_vector should be a GIN-indexed column"
            );
        }
    }
}
