use std::{error::Error, fmt, future::Future};

use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool};

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

    /// Back off after failure, doubling the delay from one minute up to an
    /// hour. Stale tokens are a no-op so they cannot release or reschedule
    /// another job's lease.
    fn release_refresh(
        &self,
        token: &str,
    ) -> impl Future<Output = Result<(), SummaryStoreError>> + Send;

    /// Fence stale jobs and reschedule without saving a new summary.
    fn skip_refresh(
        &self,
        token: &str,
    ) -> impl Future<Output = Result<(), SummaryStoreError>> + Send;

    /// Cached story notes for each prompt, in order, or `None` when missing.
    fn cached_notes(
        &self,
        model: &str,
        prompts: &[String],
    ) -> impl Future<Output = Result<Vec<Option<String>>, SummaryStoreError>> + Send;

    fn store_notes(
        &self,
        model: &str,
        prompt: &str,
        notes: &str,
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
    embedding_model: Option<String>,
}

impl PostgresSummaryRepository {
    pub(crate) fn new(pool: PgPool) -> Self {
        Self {
            pool,
            embedding_model: None,
        }
    }

    /// Read article embeddings from this model, to group near-duplicates.
    pub(crate) fn with_embedding_model(self, model: String) -> Self {
        Self {
            embedding_model: Some(model),
            ..self
        }
    }
}

/// Maps a database error to a store error describing the failed operation.
fn database(operation: &'static str) -> impl FnOnce(sqlx::Error) -> SummaryStoreError {
    move |source| SummaryStoreError::Database { operation, source }
}

/// Releases a live lease and schedules the next refresh, or fails with
/// `LeaseConflict` when the lease was lost.
async fn finish_lease(conn: &mut PgConnection, token: &str) -> Result<(), SummaryStoreError> {
    let finished = sqlx::query!(
        r#"UPDATE summary_refresh_queue
           SET lease_token = NULL, lease_expires_at = NULL,
               due_at = NOW() + INTERVAL '1 hour', failures = 0
           WHERE id = TRUE AND lease_token = $1 AND lease_expires_at > NOW()"#,
        token,
    )
    .execute(conn)
    .await
    .map_err(database("Failed to complete summary refresh"))?;
    if finished.rows_affected() == 0 {
        return Err(SummaryStoreError::LeaseConflict);
    }
    Ok(())
}

impl SummaryRepository for PostgresSummaryRepository {
    async fn latest_summary(&self) -> Result<Option<DailySummary>, SummaryStoreError> {
        sqlx::query_as!(
            DailySummary,
            r#"SELECT id, summary, feed_ids, feed_item_ids, generated_at, model, batch_model, final_model,
                      cardinality(feed_ids) AS "feed_count!", cardinality(feed_item_ids) AS "item_count!",
                      generation_ms, llm_calls, prompt_tokens, output_tokens, llm_ms, eval_ms
               FROM ai_summaries ORDER BY generated_at DESC, id DESC LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(database("Failed to fetch latest summary"))
    }

    async fn fetch_articles(&self) -> Result<Vec<SummaryArticle>, SummaryStoreError> {
        sqlx::query_as!(
            SummaryArticle,
            r#"
            SELECT i.id, i.feed_id, i.title,
                   COALESCE(f.title, f.url) AS "feed!",
                   LEFT(a.description, 300) AS "description!",
                   e.embedding AS "embedding?: pgvector::Vector"
            FROM feed_items i
            JOIN feeds f ON f.id = i.feed_id
            JOIN feed_item_details d ON d.feed_item_id = i.id
            CROSS JOIN LATERAL (
                SELECT COALESCE(NULLIF(BTRIM(d.summary), ''),
                                NULLIF(BTRIM(d.content), '')) AS description
            ) a
            LEFT JOIN feed_item_embeddings e ON e.feed_item_id = i.id AND e.model = $1
            WHERE i.inserted_at >= NOW() - INTERVAL '24 hours'
              AND i.inserted_at <= NOW()
              AND a.description IS NOT NULL
            ORDER BY i.inserted_at, i.id
            "#,
            self.embedding_model,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database("Failed to fetch recent articles"))
    }

    async fn claim_refresh(&self, force: bool) -> Result<Option<String>, SummaryStoreError> {
        sqlx::query_scalar!(
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
               RETURNING q.lease_token AS "lease_token!""#,
            LEASE_SECONDS,
            force,
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(database("Failed to claim summary refresh"))
    }

    /// Earliest time the lease can be claimed again: the schedule, or the
    /// lease expiry while a refresh is in progress.
    async fn next_due_at(&self) -> Result<DateTime<Utc>, SummaryStoreError> {
        sqlx::query_scalar!(
            r#"SELECT GREATEST(due_at, COALESCE(lease_expires_at, due_at)) AS "due_at!"
               FROM summary_refresh_queue WHERE id = TRUE"#,
        )
        .fetch_one(&self.pool)
        .await
        .map_err(database("Failed to read summary refresh schedule"))
    }

    async fn release_refresh(&self, token: &str) -> Result<(), SummaryStoreError> {
        sqlx::query!(
            r#"UPDATE summary_refresh_queue
               SET lease_token = NULL, lease_expires_at = NULL,
                   due_at = NOW() + LEAST(POWER(2, failures), 60) * INTERVAL '1 minute',
                   failures = failures + 1
               WHERE id = TRUE AND lease_token = $1"#,
            token,
        )
        .execute(&self.pool)
        .await
        .map_err(database("Failed to release summary refresh lease"))?;
        Ok(())
    }

    async fn skip_refresh(&self, token: &str) -> Result<(), SummaryStoreError> {
        let mut conn = self
            .pool
            .acquire()
            .await
            .map_err(database("Failed to skip summary refresh"))?;
        finish_lease(&mut conn, token).await
    }

    async fn cached_notes(
        &self,
        model: &str,
        prompts: &[String],
    ) -> Result<Vec<Option<String>>, SummaryStoreError> {
        sqlx::query_scalar!(
            r#"SELECT c.notes AS "notes?"
               FROM unnest($2::TEXT[]) WITH ORDINALITY AS p(prompt, ord)
               LEFT JOIN summary_note_cache c
                 ON c.prompt_hash = sha256(convert_to($1 || E'\n' || p.prompt, 'UTF8'))
               ORDER BY p.ord"#,
            model,
            prompts,
        )
        .fetch_all(&self.pool)
        .await
        .map_err(database("Failed to read summary note cache"))
    }

    async fn store_notes(
        &self,
        model: &str,
        prompt: &str,
        notes: &str,
    ) -> Result<(), SummaryStoreError> {
        sqlx::query!(
            r#"INSERT INTO summary_note_cache (prompt_hash, notes)
               VALUES (sha256(convert_to($1 || E'\n' || $2, 'UTF8')), $3)
               ON CONFLICT (prompt_hash) DO UPDATE SET notes = EXCLUDED.notes, created_at = NOW()"#,
            model,
            prompt,
            notes,
        )
        .execute(&self.pool)
        .await
        .map_err(database("Failed to store summary notes"))?;
        Ok(())
    }

    async fn complete_refresh(
        &self,
        token: &str,
        summary: &GeneratedSummary,
    ) -> Result<DailySummary, SummaryStoreError> {
        const SAVE: &str = "Failed to save summary";
        let mut tx = self.pool.begin().await.map_err(database(SAVE))?;
        finish_lease(&mut tx, token).await?;
        sqlx::query!("DELETE FROM summary_note_cache WHERE created_at < NOW() - INTERVAL '2 days'")
            .execute(&mut *tx)
            .await
            .map_err(database("Failed to prune summary note cache"))?;
        let result = sqlx::query_as!(
            DailySummary,
            r#"INSERT INTO ai_summaries (summary, feed_ids, feed_item_ids, model, batch_model,
                                         final_model, generation_ms, llm_calls, prompt_tokens,
                                         output_tokens, llm_ms, eval_ms)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
               RETURNING id, summary, feed_ids, feed_item_ids, generated_at, model, batch_model,
                         final_model, cardinality(feed_ids) AS "feed_count!",
                         cardinality(feed_item_ids) AS "item_count!",
                         generation_ms, llm_calls, prompt_tokens, output_tokens, llm_ms, eval_ms"#,
            summary.summary,
            &summary.feed_ids,
            &summary.feed_item_ids,
            summary.models.label(),
            summary.models.batch,
            summary.models.final_summary,
            summary.generation_ms,
            summary.usage.llm_calls,
            summary.usage.prompt_tokens,
            summary.usage.output_tokens,
            summary.usage.llm_ms,
            summary.usage.eval_ms,
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(database(SAVE))?;
        tx.commit().await.map_err(database(SAVE))?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::{SummaryModels, SummaryUsage};

    #[sqlx::test]
    async fn completion_persists_usage_and_reschedules_the_refresh(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let token = repository.claim_refresh(false).await.unwrap().unwrap();
        let generated = GeneratedSummary {
            summary: "Overview".into(),
            feed_ids: vec![1, 2],
            feed_item_ids: vec![10, 20],
            models: SummaryModels {
                batch: "batch".into(),
                final_summary: "final".into(),
            },
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
        assert_eq!(latest.model, "batch → final");
        assert_eq!(latest.batch_model.as_deref(), Some("batch"));
        assert_eq!(latest.final_model.as_deref(), Some("final"));
        assert_eq!((latest.feed_count, latest.item_count), (2, 2));
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
            models: SummaryModels::from_overrides(Some("model".into()), None, None),
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

    #[sqlx::test]
    async fn cached_notes_are_matched_by_model_and_prompt_in_order(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool);
        repository.store_notes("m", "first", "old").await.unwrap();
        repository
            .store_notes("m", "first", "notes 1")
            .await
            .unwrap();
        repository
            .store_notes("m", "third", "notes 3")
            .await
            .unwrap();

        let prompts = ["third", "second", "first"].map(String::from);
        let cached = repository.cached_notes("m", &prompts).await.unwrap();
        let other_model = repository.cached_notes("other", &prompts).await.unwrap();

        assert_eq!(
            cached,
            [
                Some("notes 3".to_string()),
                None,
                Some("notes 1".to_string())
            ]
        );
        assert_eq!(other_model, [None, None, None]);
    }

    #[sqlx::test]
    async fn skipping_reschedules_a_live_lease_and_rejects_a_stale_one(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let token = repository.claim_refresh(false).await.unwrap().unwrap();

        repository.skip_refresh(&token).await.unwrap();

        assert!(matches!(
            repository.skip_refresh(&token).await,
            Err(SummaryStoreError::LeaseConflict)
        ));
        let rescheduled: bool = sqlx::query_scalar(
            "SELECT lease_token IS NULL AND due_at > NOW() + INTERVAL '59 minutes'
             FROM summary_refresh_queue WHERE id = TRUE",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert!(rescheduled);
        assert!(repository.latest_summary().await.unwrap().is_none());
    }

    #[sqlx::test]
    async fn articles_carry_embeddings_from_the_configured_model_only(pool: PgPool) {
        let items: Vec<i64> = sqlx::query_scalar(
            "WITH feed AS (INSERT INTO feeds (url) VALUES ('https://example.com') RETURNING id),
                  items AS (
                      INSERT INTO feed_items (feed_id, external_id, title, url)
                      SELECT feed.id, n::TEXT, 'News', 'https://example.com' FROM feed, generate_series(1, 2) n
                      RETURNING id
                  ),
                  details AS (
                      INSERT INTO feed_item_details (feed_item_id, summary, content, author, published_at)
                      SELECT id, 'Description', '', '', NOW() FROM items
                  )
             SELECT id FROM items ORDER BY id",
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO feed_item_embeddings (feed_item_id, model, content_hash, embedding)
             VALUES ($1, 'search', 'h', '[1,0]'), ($2, 'other', 'h', '[0,1]')",
        )
        .bind(items[0])
        .bind(items[1])
        .execute(&pool)
        .await
        .unwrap();

        let embeddings = |repository: PostgresSummaryRepository| async move {
            repository
                .fetch_articles()
                .await
                .unwrap()
                .into_iter()
                .map(|article| article.embedding.map(|e| e.to_vec()))
                .collect::<Vec<_>>()
        };

        assert_eq!(
            embeddings(
                PostgresSummaryRepository::new(pool.clone()).with_embedding_model("search".into())
            )
            .await,
            [Some(vec![1.0, 0.0]), None]
        );
        assert_eq!(
            embeddings(PostgresSummaryRepository::new(pool)).await,
            [None, None]
        );
    }

    #[sqlx::test]
    async fn failures_back_off_exponentially_up_to_an_hour_and_success_resets(pool: PgPool) {
        let repository = PostgresSummaryRepository::new(pool.clone());
        let delay_minutes = || async {
            sqlx::query_scalar::<_, f64>(
                "SELECT ROUND(EXTRACT(EPOCH FROM due_at - NOW()) / 60)::FLOAT8
                 FROM summary_refresh_queue WHERE id = TRUE",
            )
            .fetch_one(&pool)
            .await
            .unwrap()
        };
        let mut delays = Vec::new();
        for _ in 0..8 {
            let token = repository.claim_refresh(true).await.unwrap().unwrap();
            repository.release_refresh(&token).await.unwrap();
            delays.push(delay_minutes().await);
        }
        assert_eq!(delays, [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 60.0, 60.0]);

        let token = repository.claim_refresh(true).await.unwrap().unwrap();
        repository.skip_refresh(&token).await.unwrap();
        let token = repository.claim_refresh(true).await.unwrap().unwrap();
        repository.release_refresh(&token).await.unwrap();
        assert_eq!(delay_minutes().await, 1.0);
    }
}
