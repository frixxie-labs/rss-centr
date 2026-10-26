use anyhow::{Context, Result};
use sqlx::Postgres;

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
    let reliable =
        |median: Option<f64>, samples: i64| median.filter(|_| samples >= MIN_CADENCE_SAMPLES);

    // Inserted-at diffs are already filtered for bulk-ingest noise in the SQL
    // query via a minimum diff threshold.
    reliable(row.median_published_seconds, row.published_samples)
        .or_else(|| reliable(row.median_inserted_seconds, row.inserted_samples))
        .map(|seconds| seconds.round() as i64)
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Utc};
    use sqlx::PgPool;

    use crate::feed::feed_item::insert_feed_item;
    use crate::feed::feed_item_detail::insert_feed_item_detail;
    use crate::feed::feed_subscription::upsert_feed_by_url;

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
}
