use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult, FeedUpdateItemInput,
};
use sqlx::{PgPool, Postgres, Transaction};

use super::feed_item::{
    insert_feed_item_dedup, insert_feed_item_detail_dedup, read_feed_cadence_seconds,
};
use super::feed_subscription::FeedSuccessUpdate;
use super::ingest::{backoff_poll_interval_seconds, resolved_poll_interval_seconds};

const LEASE_CONFLICT_MESSAGE: &str = "feed update lease conflict";
const FEED_PAUSED_MESSAGE: &str = "feed is paused";

struct FeedPollInterval {
    poll_interval_seconds: i64,
}

pub fn is_lease_conflict(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().starts_with(LEASE_CONFLICT_MESSAGE))
}

pub fn is_feed_paused(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|cause| cause.to_string().starts_with(FEED_PAUSED_MESSAGE))
}

pub async fn enqueue_feed_now(pool: &PgPool, feed_id: i64) -> Result<()> {
    let feed = sqlx::query!(
        r#"
        SELECT is_enabled as "is_enabled!: bool"
        FROM feeds
        WHERE id = $1
        "#,
        feed_id,
    )
    .fetch_optional(pool)
    .await
    .with_context(|| format!("failed to check feed status for feed_id={feed_id}"))?;

    let Some(feed) = feed else {
        anyhow::bail!("no feed found with id={feed_id}");
    };

    if !feed.is_enabled {
        anyhow::bail!("{FEED_PAUSED_MESSAGE} for feed_id={feed_id}");
    }

    let result = sqlx::query!(
        r#"
        INSERT INTO feed_update_queue (feed_id, due_at)
        VALUES ($1, NOW())
        ON CONFLICT (feed_id)
        DO UPDATE SET due_at = LEAST(feed_update_queue.due_at, excluded.due_at),
                      updated_at = NOW()
        "#,
        feed_id,
    )
    .execute(pool)
    .await
    .with_context(|| format!("failed to enqueue feed update for feed_id={feed_id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={feed_id}");
    }

    Ok(())
}

pub async fn dequeue_due_feeds(
    pool: &PgPool,
    limit: i64,
    lease_seconds: i64,
) -> Result<Vec<DequeuedFeedUpdate>> {
    let rows = sqlx::query_as!(
        DequeuedFeedUpdate,
        r#"
        WITH due AS (
            SELECT q.feed_id
            FROM feed_update_queue q
            JOIN feeds f ON f.id = q.feed_id
            WHERE f.is_enabled = TRUE
              AND q.due_at <= NOW()
              AND (
                  q.lease_expires_at IS NULL
                  OR q.lease_expires_at <= NOW()
              )
            ORDER BY q.due_at ASC, q.feed_id ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
        ), leased AS (
            UPDATE feed_update_queue q
            SET leased_at = NOW(),
                lease_expires_at = NOW() + ($2::BIGINT * INTERVAL '1 second'),
                lease_token = md5(random()::TEXT || clock_timestamp()::TEXT || q.feed_id::TEXT),
                attempts = attempts + 1,
                updated_at = NOW()
            FROM due
            WHERE q.feed_id = due.feed_id
            RETURNING q.feed_id,
                      q.lease_token,
                      q.lease_expires_at
        )
        SELECT l.feed_id as "feed_id!: i64",
               f.url,
               f.title,
               f.site_url,
               f.etag,
               f.last_modified,
               f.poll_interval_seconds,
               f.last_checked_at as "last_checked_at: _",
               f.last_success_at as "last_success_at: _",
               f.last_inserted_at as "last_inserted_at: _",
               f.failure_count,
               l.lease_token as "lease_token!: String",
               l.lease_expires_at as "lease_expires_at!: _"
        FROM leased l
        JOIN feeds f ON f.id = l.feed_id
        ORDER BY l.feed_id ASC
        "#,
        limit,
        lease_seconds,
    )
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!(
            "failed to dequeue due feed updates with limit={limit} lease_seconds={lease_seconds}"
        )
    })?;

    Ok(rows)
}

pub async fn complete_feed_update(
    pool: &PgPool,
    feed_id: i64,
    request: CompleteFeedUpdateRequest,
) -> Result<CompleteFeedUpdateResult> {
    let completed_at = Utc::now();
    let mut tx = pool
        .begin()
        .await
        .with_context(|| format!("failed to begin feed update completion for feed_id={feed_id}"))?;

    ensure_current_lease(&mut tx, feed_id, &request.lease_token).await?;

    let mut inserted_items = 0i64;
    for item in &request.items {
        let inserted = insert_feed_item_dedup(
            &mut *tx,
            feed_id,
            item.external_id.as_str(),
            item.title.as_str(),
            item.url.as_str(),
        )
        .await?;

        let Some(inserted) = inserted else {
            continue;
        };
        inserted_items += 1;

        if item_has_detail(item) {
            let summary = item.summary.as_deref().unwrap_or("");
            let content = item.content.as_deref().unwrap_or("");
            let author = item.author.as_deref().unwrap_or("");
            let published_at = item.published_at.unwrap_or(completed_at);

            let _detail = insert_feed_item_detail_dedup(
                &mut *tx,
                inserted.id,
                summary,
                content,
                author,
                published_at,
            )
            .await
            .with_context(|| format!("failed to insert detail for feed_item_id={}", inserted.id))?;
        }
    }

    let poll_interval_seconds = if request.fetched {
        resolved_poll_interval_seconds(read_feed_cadence_seconds(&mut *tx, feed_id).await?)
    } else {
        let current = read_feed_poll_interval(&mut tx, feed_id).await?;
        backoff_poll_interval_seconds(current.poll_interval_seconds)
    };
    let last_inserted_at = (inserted_items > 0).then_some(completed_at);
    touch_feed_success_in_tx(
        &mut tx,
        feed_id,
        FeedSuccessUpdate {
            checked_at: completed_at,
            title: request.title.as_deref(),
            site_url: request.site_url.as_deref(),
            etag: request.etag.as_deref(),
            last_modified: request.last_modified.as_deref(),
            poll_interval_seconds: Some(poll_interval_seconds),
            last_inserted_at,
        },
    )
    .await?;

    let next_due_at = completed_at + Duration::seconds(poll_interval_seconds);
    let result = sqlx::query!(
        r#"
        UPDATE feed_update_queue
        SET due_at = $1,
            leased_at = NULL,
            lease_expires_at = NULL,
            lease_token = NULL,
            attempts = 0,
            updated_at = NOW()
        WHERE feed_id = $2
          AND lease_token = $3
        "#,
        next_due_at,
        feed_id,
        request.lease_token,
    )
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to reschedule completed feed update for feed_id={feed_id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("{LEASE_CONFLICT_MESSAGE} for feed_id={feed_id}");
    }

    tx.commit().await.with_context(|| {
        format!("failed to commit feed update completion for feed_id={feed_id}")
    })?;

    Ok(CompleteFeedUpdateResult {
        inserted_items,
        next_due_at,
    })
}

pub async fn fail_feed_update(
    pool: &PgPool,
    feed_id: i64,
    request: FailedFeedUpdateRequest,
) -> Result<FailedFeedUpdateResult> {
    let checked_at = Utc::now();
    let mut tx = pool
        .begin()
        .await
        .with_context(|| format!("failed to begin feed update failure for feed_id={feed_id}"))?;

    ensure_current_lease(&mut tx, feed_id, &request.lease_token).await?;
    let current = read_feed_poll_interval(&mut tx, feed_id).await?;
    let poll_interval_seconds = backoff_poll_interval_seconds(current.poll_interval_seconds);
    touch_feed_failure_in_tx(&mut tx, feed_id, checked_at, poll_interval_seconds).await?;

    let next_due_at = checked_at + Duration::seconds(poll_interval_seconds);
    let result = sqlx::query!(
        r#"
        UPDATE feed_update_queue
        SET due_at = $1,
            leased_at = NULL,
            lease_expires_at = NULL,
            lease_token = NULL,
            updated_at = NOW()
        WHERE feed_id = $2
          AND lease_token = $3
        "#,
        next_due_at,
        feed_id,
        request.lease_token,
    )
    .execute(&mut *tx)
    .await
    .with_context(|| format!("failed to reschedule failed feed update for feed_id={feed_id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("{LEASE_CONFLICT_MESSAGE} for feed_id={feed_id}");
    }

    tx.commit()
        .await
        .with_context(|| format!("failed to commit feed update failure for feed_id={feed_id}"))?;

    Ok(FailedFeedUpdateResult { next_due_at })
}

async fn ensure_current_lease(
    tx: &mut Transaction<'_, Postgres>,
    feed_id: i64,
    lease_token: &str,
) -> Result<()> {
    let row = sqlx::query!(
        r#"
        SELECT feed_id
        FROM feed_update_queue
        WHERE feed_id = $1
          AND lease_token = $2
          AND lease_expires_at > NOW()
        FOR UPDATE
        "#,
        feed_id,
        lease_token,
    )
    .fetch_optional(&mut **tx)
    .await
    .with_context(|| format!("failed to verify feed update lease for feed_id={feed_id}"))?;

    if row.is_none() {
        anyhow::bail!("{LEASE_CONFLICT_MESSAGE} for feed_id={feed_id}");
    }

    Ok(())
}

async fn touch_feed_success_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    feed_id: i64,
    update: FeedSuccessUpdate<'_>,
) -> Result<()> {
    let FeedSuccessUpdate {
        checked_at,
        title,
        site_url,
        etag,
        last_modified,
        poll_interval_seconds,
        last_inserted_at,
    } = update;

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
            last_inserted_at = COALESCE($7, last_inserted_at),
            failure_count = 0
        WHERE id = $8
        "#,
        title,
        site_url,
        etag,
        last_modified,
        poll_interval_seconds,
        checked_at,
        last_inserted_at,
        feed_id,
    )
    .execute(&mut **tx)
    .await
    .with_context(|| format!("failed to update feed success for feed_id={feed_id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={feed_id}");
    }

    Ok(())
}

async fn touch_feed_failure_in_tx(
    tx: &mut Transaction<'_, Postgres>,
    feed_id: i64,
    checked_at: DateTime<Utc>,
    poll_interval_seconds: i64,
) -> Result<()> {
    let result = sqlx::query!(
        r#"
        UPDATE feeds
        SET last_checked_at = $1,
            failure_count = failure_count + 1,
            poll_interval_seconds = $2
        WHERE id = $3
        "#,
        checked_at,
        poll_interval_seconds,
        feed_id,
    )
    .execute(&mut **tx)
    .await
    .with_context(|| format!("failed to update feed failure for feed_id={feed_id}"))?;

    if result.rows_affected() == 0 {
        anyhow::bail!("no feed found with id={feed_id}");
    }

    Ok(())
}

async fn read_feed_poll_interval(
    tx: &mut Transaction<'_, Postgres>,
    feed_id: i64,
) -> Result<FeedPollInterval> {
    let row = sqlx::query_as!(
        FeedPollInterval,
        r#"
        SELECT poll_interval_seconds
        FROM feeds
        WHERE id = $1
        "#,
        feed_id,
    )
    .fetch_optional(&mut **tx)
    .await
    .with_context(|| format!("failed to read feed poll interval for feed_id={feed_id}"))?;

    let Some(row) = row else {
        anyhow::bail!("no feed found with id={feed_id}");
    };

    Ok(row)
}

fn item_has_detail(item: &FeedUpdateItemInput) -> bool {
    item.summary.is_some()
        || item.content.is_some()
        || item.author.is_some()
        || item.published_at.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_item::read_feed_items_by_feed;
    use crate::feed::feed_subscription::{read_feed, set_feed_enabled, upsert_feed_by_url};

    #[sqlx::test]
    async fn test_dequeue_leases_due_feed_once(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-once.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();

        let first = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        assert!(first.iter().any(|row| row.feed_id == feed.id));

        let second = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        assert!(second.iter().all(|row| row.feed_id != feed.id));
    }

    #[sqlx::test]
    async fn test_dequeue_returns_expired_lease(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-expired.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();

        let first = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        let leased = first.iter().find(|row| row.feed_id == feed.id).unwrap();

        sqlx::query(
            r#"
            UPDATE feed_update_queue
            SET lease_expires_at = NOW() - INTERVAL '1 second'
            WHERE feed_id = $1
            "#,
        )
        .bind(feed.id)
        .execute(&pool)
        .await
        .unwrap();

        let second = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        let leased_again = second.iter().find(|row| row.feed_id == feed.id).unwrap();
        assert_ne!(leased.lease_token, leased_again.lease_token);
    }

    #[sqlx::test]
    async fn test_complete_feed_update_inserts_items_and_reschedules(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-complete.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();
        let leased = dequeue_due_feeds(&pool, 100, 300)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.feed_id == feed.id)
            .unwrap();

        let result = complete_feed_update(
            &pool,
            feed.id,
            CompleteFeedUpdateRequest {
                lease_token: leased.lease_token,
                fetched: true,
                title: Some("Example Feed".to_string()),
                site_url: Some("https://example.com".to_string()),
                etag: Some("\"etag\"".to_string()),
                last_modified: Some("Wed, 01 Jul 2026 10:00:00 GMT".to_string()),
                items: vec![FeedUpdateItemInput {
                    external_id: "item-1".to_string(),
                    title: "One".to_string(),
                    url: "https://example.com/1".to_string(),
                    summary: Some("Summary".to_string()),
                    content: None,
                    author: None,
                    published_at: None,
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(result.inserted_items, 1);
        let items = read_feed_items_by_feed(&pool, feed.id).await.unwrap();
        assert_eq!(items.len(), 1);

        let updated = read_feed(&pool, feed.id).await.unwrap();
        assert!(updated.last_inserted_at.is_some());
        assert_eq!(updated.etag.as_deref(), Some("\"etag\""));
        assert_eq!(updated.title.as_deref(), Some("Example Feed"));
        assert_eq!(updated.site_url.as_deref(), Some("https://example.com"));

        let due_again = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        assert!(due_again.iter().all(|row| row.feed_id != feed.id));
    }

    #[sqlx::test]
    async fn test_complete_feed_update_rejects_stale_token(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-stale.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();
        let leased = dequeue_due_feeds(&pool, 100, 300)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.feed_id == feed.id)
            .unwrap();

        let err = complete_feed_update(
            &pool,
            feed.id,
            CompleteFeedUpdateRequest {
                lease_token: format!("{}-stale", leased.lease_token),
                fetched: false,
                title: None,
                site_url: None,
                etag: None,
                last_modified: None,
                items: Vec::new(),
            },
        )
        .await
        .unwrap_err();

        assert!(is_lease_conflict(&err));
    }

    #[sqlx::test]
    async fn test_failed_feed_update_backs_off_and_clears_lease(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-failed.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();
        let leased = dequeue_due_feeds(&pool, 100, 300)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.feed_id == feed.id)
            .unwrap();

        fail_feed_update(
            &pool,
            feed.id,
            FailedFeedUpdateRequest {
                lease_token: leased.lease_token,
            },
        )
        .await
        .unwrap();

        let updated = read_feed(&pool, feed.id).await.unwrap();
        assert_eq!(updated.failure_count, 1);
        assert_eq!(
            updated.poll_interval_seconds,
            feed.poll_interval_seconds * 2
        );

        let due_again = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        assert!(due_again.iter().all(|row| row.feed_id != feed.id));
    }

    #[sqlx::test]
    async fn test_disabled_feed_is_not_dequeued(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-disabled.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();
        set_feed_enabled(&pool, feed.id, false).await.unwrap();

        let due = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        assert!(due.iter().all(|row| row.feed_id != feed.id));
    }

    #[sqlx::test]
    async fn test_enqueue_feed_now_rejects_paused_feed(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-paused.xml")
            .await
            .unwrap();
        set_feed_enabled(&pool, feed.id, false).await.unwrap();

        let err = enqueue_feed_now(&pool, feed.id).await.unwrap_err();
        assert!(is_feed_paused(&err));

        let due = dequeue_due_feeds(&pool, 100, 300).await.unwrap();
        assert!(due.iter().all(|row| row.feed_id != feed.id));
    }

    #[sqlx::test]
    async fn test_complete_feed_update_skips_last_inserted_at_without_new_items(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/queue-no-new-items.xml")
            .await
            .unwrap();
        enqueue_feed_now(&pool, feed.id).await.unwrap();
        let leased = dequeue_due_feeds(&pool, 100, 300)
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.feed_id == feed.id)
            .unwrap();

        let result = complete_feed_update(
            &pool,
            feed.id,
            CompleteFeedUpdateRequest {
                lease_token: leased.lease_token,
                fetched: true,
                title: Some("Example Feed".to_string()),
                site_url: None,
                etag: None,
                last_modified: None,
                items: Vec::new(),
            },
        )
        .await
        .unwrap();

        assert_eq!(result.inserted_items, 0);
        let updated = read_feed(&pool, feed.id).await.unwrap();
        assert!(updated.last_inserted_at.is_none());
        assert!(updated.last_success_at.is_some());
        assert_eq!(updated.title.as_deref(), Some("Example Feed"));
    }
}
