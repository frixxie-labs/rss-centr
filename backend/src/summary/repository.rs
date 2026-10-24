use std::{error::Error, fmt, future::Future};

use chrono::{DateTime, Utc};
use sqlx::PgPool;

use super::{DailySummary, GeneratedSummary, SummaryArticle};

pub(crate) const LEASE_SECONDS: i64 = 60 * 60;

#[derive(Debug)]
pub(crate) enum SummaryStoreError {
    Database {
        operation: &'static str,
        source: sqlx::Error,
    },
    LeaseConflict,
}

impl fmt::Display for SummaryStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database { operation, .. } => f.write_str(operation),
            Self::LeaseConflict => f.write_str("Summary refresh lease conflict"),
        }
    }
}

impl Error for SummaryStoreError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Database { source, .. } => Some(source),
            Self::LeaseConflict => None,
        }
    }
}

pub(crate) trait SummaryRepository: Send + Sync {
    fn latest_summary(
        &self,
    ) -> impl Future<Output = Result<Option<DailySummary>, SummaryStoreError>> + Send;

    fn fetch_articles(
        &self,
    ) -> impl Future<Output = Result<Vec<SummaryArticle>, SummaryStoreError>> + Send;

    fn claim_refresh(
        &self,
        force: bool,
    ) -> impl Future<Output = Result<Option<String>, SummaryStoreError>> + Send;

    fn next_due_at(&self) -> impl Future<Output = Result<DateTime<Utc>, SummaryStoreError>> + Send;

    /// Back off after failure. Stale tokens are a no-op so they cannot release
    /// or reschedule another job's lease.
    fn release_refresh(
        &self,
        token: &str,
    ) -> impl Future<Output = Result<(), SummaryStoreError>> + Send;

    /// Fence stale jobs and reschedule atomically with persistence.
    fn complete_refresh(
        &self,
        token: &str,
        summary: &GeneratedSummary,
    ) -> impl Future<Output = Result<DailySummary, SummaryStoreError>> + Send;
}

#[derive(Clone)]
pub(crate) struct PostgresSummaryRepository {
    pool: PgPool,
}

impl PostgresSummaryRepository {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl SummaryRepository for PostgresSummaryRepository {
    async fn latest_summary(&self) -> Result<Option<DailySummary>, SummaryStoreError> {
        sqlx::query_as::<_, DailySummary>(
            r#"SELECT id, summary, feed_ids, feed_item_ids, generated_at, model,
                      generation_ms, llm_calls, prompt_tokens, output_tokens, llm_ms, eval_ms
               FROM ai_summaries ORDER BY generated_at DESC, id DESC LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to fetch latest summary",
            source,
        })
    }

    async fn fetch_articles(&self) -> Result<Vec<SummaryArticle>, SummaryStoreError> {
        sqlx::query_as::<_, SummaryArticle>(
            r#"
            SELECT i.id, i.feed_id, i.title, COALESCE(f.title, f.url) AS feed,
                   LEFT(COALESCE(NULLIF(BTRIM(d.summary), ''),
                                 NULLIF(BTRIM(d.content), '')), 300) AS description
            FROM feed_items i
            JOIN feeds f ON f.id = i.feed_id
            JOIN feed_item_details d ON d.feed_item_id = i.id
            WHERE i.inserted_at >= NOW() - INTERVAL '24 hours'
              AND i.inserted_at <= NOW()
              AND COALESCE(NULLIF(BTRIM(d.summary), ''),
                           NULLIF(BTRIM(d.content), '')) IS NOT NULL
            ORDER BY i.inserted_at DESC, i.id DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to fetch recent articles",
            source,
        })
    }

    async fn claim_refresh(&self, force: bool) -> Result<Option<String>, SummaryStoreError> {
        sqlx::query_scalar(
            r#"WITH due AS (
                   SELECT id FROM summary_refresh_queue
                   WHERE ($2 OR due_at <= NOW())
                     AND (lease_expires_at IS NULL OR lease_expires_at <= NOW())
                   FOR UPDATE SKIP LOCKED
               )
               UPDATE summary_refresh_queue q
               SET lease_token = md5(random()::TEXT || clock_timestamp()::TEXT),
                   lease_expires_at = NOW() + ($1::BIGINT * INTERVAL '1 second')
               FROM due WHERE q.id = due.id
               RETURNING q.lease_token"#,
        )
        .bind(LEASE_SECONDS)
        .bind(force)
        .fetch_optional(&self.pool)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to claim summary refresh",
            source,
        })
    }

    /// Earliest time the lease can be claimed again: the schedule, or the
    /// lease expiry while a refresh is in progress.
    async fn next_due_at(&self) -> Result<DateTime<Utc>, SummaryStoreError> {
        sqlx::query_scalar(
            r#"SELECT GREATEST(due_at, COALESCE(lease_expires_at, due_at))
               FROM summary_refresh_queue WHERE id = TRUE"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to read summary refresh schedule",
            source,
        })
    }

    async fn release_refresh(&self, token: &str) -> Result<(), SummaryStoreError> {
        sqlx::query(
            r#"UPDATE summary_refresh_queue
               SET lease_token = NULL, lease_expires_at = NULL,
                   due_at = NOW() + INTERVAL '1 minute'
               WHERE id = TRUE AND lease_token = $1"#,
        )
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to release summary refresh lease",
            source,
        })?;
        Ok(())
    }

    async fn complete_refresh(
        &self,
        token: &str,
        summary: &GeneratedSummary,
    ) -> Result<DailySummary, SummaryStoreError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|source| SummaryStoreError::Database {
                operation: "Failed to save summary",
                source,
            })?;
        let completed = sqlx::query(
            r#"UPDATE summary_refresh_queue
               SET lease_token = NULL, lease_expires_at = NULL,
                   due_at = NOW() + INTERVAL '1 hour'
               WHERE id = TRUE AND lease_token = $1 AND lease_expires_at > NOW()"#,
        )
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to complete summary refresh",
            source,
        })?;
        if completed.rows_affected() == 0 {
            return Err(SummaryStoreError::LeaseConflict);
        }
        let result = sqlx::query_as::<_, DailySummary>(
            r#"INSERT INTO ai_summaries (summary, feed_ids, feed_item_ids, model, generation_ms,
                                         llm_calls, prompt_tokens, output_tokens, llm_ms, eval_ms)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
               RETURNING id, summary, feed_ids, feed_item_ids, generated_at, model,
                         generation_ms, llm_calls, prompt_tokens, output_tokens, llm_ms, eval_ms"#,
        )
        .bind(&summary.summary)
        .bind(&summary.feed_ids)
        .bind(&summary.feed_item_ids)
        .bind(&summary.model)
        .bind(summary.generation_ms)
        .bind(summary.usage.llm_calls)
        .bind(summary.usage.prompt_tokens)
        .bind(summary.usage.output_tokens)
        .bind(summary.usage.llm_ms)
        .bind(summary.usage.eval_ms)
        .fetch_one(&mut *tx)
        .await
        .map_err(|source| SummaryStoreError::Database {
            operation: "Failed to save summary",
            source,
        })?;
        tx.commit()
            .await
            .map_err(|source| SummaryStoreError::Database {
                operation: "Failed to save summary",
                source,
            })?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::SummaryUsage;

    #[sqlx::test]
    async fn completion_persists_usage_and_reschedules_the_refresh(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let token = repository.claim_refresh(false).await.unwrap().unwrap();
        let generated = GeneratedSummary {
            summary: "Overview".into(),
            feed_ids: vec![1, 2],
            feed_item_ids: vec![10, 20],
            model: "batch → final".into(),
            generation_ms: 900,
            usage: SummaryUsage {
                llm_calls: 3,
                prompt_tokens: 1200,
                output_tokens: 300,
                llm_ms: 800,
                eval_ms: 600,
            },
        };

        let saved = repository
            .complete_refresh(&token, &generated)
            .await
            .unwrap();

        let latest = repository.latest_summary().await.unwrap().unwrap();
        assert_eq!(latest.id, saved.id);
        assert_eq!(latest.summary, generated.summary);
        assert_eq!(latest.feed_ids, generated.feed_ids);
        assert_eq!(latest.feed_item_ids, generated.feed_item_ids);
        assert_eq!(latest.model, generated.model);
        assert_eq!(latest.generation_ms, Some(900));
        assert_eq!(latest.llm_calls, Some(3));
        assert_eq!(latest.prompt_tokens, Some(1200));
        assert_eq!(latest.output_tokens, Some(300));
        assert_eq!(latest.llm_ms, Some(800));
        assert_eq!(latest.eval_ms, Some(600));
        let (released, scheduled): (bool, bool) = sqlx::query_as(
            "SELECT lease_token IS NULL AND lease_expires_at IS NULL,
                    due_at BETWEEN NOW() + INTERVAL '59 minutes' AND NOW() + INTERVAL '1 hour'
             FROM summary_refresh_queue WHERE id = TRUE",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(released);
        assert!(scheduled);
        assert!(repository.claim_refresh(false).await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn failed_save_rolls_back_lease_completion_and_preserves_previous_summary(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let generated = GeneratedSummary {
            summary: "Previous overview".into(),
            feed_ids: vec![1],
            feed_item_ids: vec![10],
            model: "model".into(),
            generation_ms: 1,
            usage: SummaryUsage::default(),
        };
        let previous_token = repository.claim_refresh(false).await.unwrap().unwrap();
        let previous = repository
            .complete_refresh(&previous_token, &generated)
            .await
            .unwrap();
        let token = repository.claim_refresh(true).await.unwrap().unwrap();
        let before: (DateTime<Utc>, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT due_at, lease_token, lease_expires_at FROM summary_refresh_queue WHERE id = TRUE",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        // Fail the insert after the lease update has run inside the transaction.
        sqlx::query(
            "ALTER TABLE ai_summaries ADD CONSTRAINT reject_new_summary CHECK (summary <> 'Rejected')",
        )
        .execute(&pool)
        .await
        .unwrap();
        let rejected = GeneratedSummary {
            summary: "Rejected".into(),
            ..generated
        };

        let result = repository.complete_refresh(&token, &rejected).await;

        assert!(matches!(result, Err(SummaryStoreError::Database { .. })));
        let after: (DateTime<Utc>, Option<String>, Option<DateTime<Utc>>) = sqlx::query_as(
            "SELECT due_at, lease_token, lease_expires_at FROM summary_refresh_queue WHERE id = TRUE",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(after, before);
        let latest = repository.latest_summary().await.unwrap().unwrap();
        assert_eq!(latest.id, previous.id);
        assert_eq!(latest.summary, "Previous overview");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ai_summaries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[sqlx::test]
    async fn stale_release_does_not_clear_a_replacement_lease(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let stale = repository.claim_refresh(false).await.unwrap().unwrap();
        sqlx::query(
            "UPDATE summary_refresh_queue SET lease_expires_at = NOW() - INTERVAL '1 second'",
        )
        .execute(&pool)
        .await
        .unwrap();
        let current = repository.claim_refresh(false).await.unwrap().unwrap();
        let schedule_before = repository.next_due_at().await.unwrap();

        repository.release_refresh(&stale).await.unwrap();

        assert_eq!(repository.next_due_at().await.unwrap(), schedule_before);
        let token: Option<String> =
            sqlx::query_scalar("SELECT lease_token FROM summary_refresh_queue WHERE id = TRUE")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(token.as_deref(), Some(current.as_str()));
        assert!(repository.claim_refresh(true).await.unwrap().is_none());
    }
}
