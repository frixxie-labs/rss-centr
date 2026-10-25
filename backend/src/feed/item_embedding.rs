use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rss_centr_core::semantic::{
    EmbeddingJob, ItemEmbedding, content_hash, embedding_input, normalize,
};
use sqlx::PgPool;

use crate::search::{EmbeddingSource, EmbeddingVersion, StoredEmbedding};

use super::feed_item::FeedItemWithDetail;

/// Lease up to `limit` items that have no embedding for `model` (missing or
/// produced by a different model), newest first. Leased items are not handed
/// out again until the lease expires or an embedding is stored.
pub async fn claim_items_needing_embedding(
    pool: &PgPool,
    model: &str,
    limit: i64,
    lease_seconds: i64,
) -> Result<Vec<EmbeddingJob>> {
    let rows = sqlx::query!(
        r#"
        WITH candidates AS (
            SELECT f.id
            FROM feed_items f
            LEFT JOIN feed_item_embeddings e ON e.feed_item_id = f.id AND e.model = $1
            LEFT JOIN feed_item_embedding_leases l ON l.feed_item_id = f.id
            WHERE e.feed_item_id IS NULL
              AND (l.leased_until IS NULL OR l.leased_until < NOW())
            ORDER BY f.id DESC
            LIMIT $2
            FOR NO KEY UPDATE OF f SKIP LOCKED
        ),
        leased AS (
            INSERT INTO feed_item_embedding_leases (feed_item_id, leased_until)
            SELECT id, NOW() + make_interval(secs => $3::FLOAT8) FROM candidates
            ON CONFLICT (feed_item_id) DO UPDATE
                SET leased_until = excluded.leased_until
                WHERE feed_item_embedding_leases.leased_until < NOW()
            RETURNING feed_item_id
        )
        SELECT f.id as "id!: i64", f.title, d.summary as "summary?", d.content as "content?"
        FROM leased
        INNER JOIN feed_items f ON f.id = leased.feed_item_id
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        ORDER BY f.id DESC
        "#,
        model,
        limit,
        lease_seconds as f64,
    )
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!("failed to claim items needing embedding for model={model} limit={limit}")
    })?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let input = embedding_input(&row.title, row.summary.as_deref(), row.content.as_deref());
            EmbeddingJob {
                feed_item_id: row.id,
                content_hash: content_hash(&input),
                input,
            }
        })
        .collect())
}

/// Upsert embeddings for `model` and release their leases. Vectors are
/// normalized before storage; items that no longer exist are skipped.
/// Returns the number of embeddings stored.
pub async fn store_item_embeddings(
    pool: &PgPool,
    model: &str,
    embeddings: &[ItemEmbedding],
) -> Result<u64> {
    // Store everything in one statement: the database is a network hop away,
    // so per-item round-trips dominate the cost.
    let mut ids = Vec::with_capacity(embeddings.len());
    let mut hashes = Vec::with_capacity(embeddings.len());
    let mut vectors = Vec::with_capacity(embeddings.len());
    for item in embeddings {
        let Some(vector) = normalize(&item.embedding) else {
            bail!(
                "invalid embedding for feed_item_id={}: empty or degenerate vector",
                item.feed_item_id
            );
        };
        ids.push(item.feed_item_id);
        hashes.push(item.content_hash.clone());
        vectors.push(encode_embedding(&vector));
    }

    let stored = sqlx::query_scalar!(
        r#"
        WITH input AS (
            SELECT DISTINCT ON (feed_item_id) *
            FROM UNNEST($2::BIGINT[], $3::TEXT[], $4::BYTEA[]) WITH ORDINALITY
                AS t(feed_item_id, content_hash, embedding, ord)
            ORDER BY feed_item_id, ord DESC
        ),
        released AS (
            DELETE FROM feed_item_embedding_leases
            WHERE feed_item_id IN (SELECT feed_item_id FROM input)
        ),
        stored AS (
            INSERT INTO feed_item_embeddings (feed_item_id, model, content_hash, embedding, embedded_at)
            SELECT f.id, $1, i.content_hash, i.embedding, NOW()
            FROM input i
            INNER JOIN feed_items f ON f.id = i.feed_item_id
            ON CONFLICT (feed_item_id) DO UPDATE
                SET model = excluded.model,
                    content_hash = excluded.content_hash,
                    embedding = excluded.embedding,
                    embedded_at = excluded.embedded_at
            RETURNING 1
        )
        SELECT COUNT(*) as "count!" FROM stored
        "#,
        model,
        &ids,
        &hashes,
        &vectors,
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to store {} embeddings", ids.len()))?;
    Ok(stored as u64)
}

/// Pack a vector as big-endian f32 bytes, the layout of Postgres `float4send`.
pub fn encode_embedding(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_be_bytes()).collect()
}

/// Inverse of [`encode_embedding`]. Trailing partial values are ignored.
pub fn decode_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| f32::from_be_bytes(*chunk))
        .collect()
}

impl EmbeddingSource for PgPool {
    /// Read embeddings for `model`, optionally only those stored after `since`.
    async fn read_embeddings(
        &self,
        model: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<Vec<StoredEmbedding>> {
        let rows = sqlx::query!(
            r#"
        SELECT e.feed_item_id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               LEAST(d.published_at, f.inserted_at) as "date!: DateTime<Utc>",
               e.embedding as "embedding!",
               e.embedded_at as "embedded_at!"
        FROM feed_item_embeddings e
        INNER JOIN feed_items f ON f.id = e.feed_item_id
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        WHERE e.model = $1
          AND ($2::TIMESTAMPTZ IS NULL OR e.embedded_at > $2)
        "#,
            model,
            since,
        )
        .fetch_all(self)
        .await
        .with_context(|| format!("failed to read embeddings for model={model} since={since:?}"))?;

        Ok(rows
            .into_iter()
            .map(|row| StoredEmbedding {
                id: row.id,
                feed_id: row.feed_id,
                date: row.date,
                embedding: decode_embedding(&row.embedding),
                embedded_at: row.embedded_at,
            })
            .collect())
    }

    /// Read embeddings for `model` of the given items.
    async fn read_embeddings_by_ids(
        &self,
        model: &str,
        ids: &[i64],
    ) -> Result<Vec<StoredEmbedding>> {
        let rows = sqlx::query!(
            r#"
        SELECT e.feed_item_id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               LEAST(d.published_at, f.inserted_at) as "date!: DateTime<Utc>",
               e.embedding as "embedding!",
               e.embedded_at as "embedded_at!"
        FROM feed_item_embeddings e
        INNER JOIN feed_items f ON f.id = e.feed_item_id
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        WHERE e.model = $1
          AND e.feed_item_id = ANY($2)
        "#,
            model,
            ids,
        )
        .fetch_all(self)
        .await
        .with_context(|| {
            format!(
                "failed to read embeddings for model={model} count={}",
                ids.len()
            )
        })?;

        Ok(rows
            .into_iter()
            .map(|row| StoredEmbedding {
                id: row.id,
                feed_id: row.feed_id,
                date: row.date,
                embedding: decode_embedding(&row.embedding),
                embedded_at: row.embedded_at,
            })
            .collect())
    }

    /// Read the stored embeddings for `model` without their vectors, which make
    /// up nearly all of the data.
    async fn read_embedding_versions(&self, model: &str) -> Result<Vec<EmbeddingVersion>> {
        sqlx::query_as!(
            EmbeddingVersion,
            r#"
        SELECT e.feed_item_id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               LEAST(d.published_at, f.inserted_at) as "date!: DateTime<Utc>",
               e.embedded_at as "embedded_at!"
        FROM feed_item_embeddings e
        INNER JOIN feed_items f ON f.id = e.feed_item_id
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        WHERE e.model = $1
        "#,
            model,
        )
        .fetch_all(self)
        .await
        .with_context(|| format!("failed to read embedding versions for model={model}"))
    }
}

/// Read items with details, returned in the order of `ids`. Unknown ids are skipped.
pub async fn read_feed_items_with_detail_by_ids(
    pool: &PgPool,
    ids: &[i64],
) -> Result<Vec<FeedItemWithDetail>> {
    let rows = sqlx::query_as!(
        FeedItemWithDetail,
        r#"
        SELECT f.id as "id!: i64",
               f.feed_id as "feed_id!: i64",
               f.external_id as "external_id!",
               f.title as "title!",
               f.url as "url!",
               f.inserted_at as "inserted_at!: _",
               d.summary as "summary?",
               d.content as "content?",
               d.author as "author?",
               d.published_at as "published_at?: _"
        FROM feed_items f
        LEFT JOIN feed_item_details d ON d.feed_item_id = f.id
        WHERE f.id = ANY($1)
        "#,
        ids,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read feed items by ids count={}", ids.len()))?;

    let mut by_id: HashMap<i64, FeedItemWithDetail> =
        rows.into_iter().map(|row| (row.id, row)).collect();
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_item::{insert_feed_item, insert_feed_item_detail};
    use crate::feed::feed_subscription::upsert_feed_by_url;
    use crate::search::{EmbeddingIndex, ReconcileStats};

    const MODEL: &str = "test-model";

    async fn item(pool: &PgPool, feed_id: i64, key: &str, summary: &str) -> i64 {
        let item = insert_feed_item(
            pool,
            feed_id,
            key,
            &format!("Title {key}"),
            &format!("https://example.com/{key}"),
        )
        .await
        .unwrap();
        insert_feed_item_detail(pool, item.id, summary, "", "", Utc::now())
            .await
            .unwrap();
        item.id
    }

    fn embedding(feed_item_id: i64, vector: &[f32]) -> ItemEmbedding {
        ItemEmbedding {
            feed_item_id,
            content_hash: "hash".to_string(),
            embedding: vector.to_vec(),
        }
    }

    #[sqlx::test]
    async fn claim_returns_unembedded_items_newest_first_with_input(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let first = item(&pool, feed.id, "one", "<p>First summary</p>").await;
        let second = item(&pool, feed.id, "two", "Second summary").await;

        let jobs = claim_items_needing_embedding(&pool, MODEL, 10, 60)
            .await
            .unwrap();
        let ids: Vec<i64> = jobs.iter().map(|j| j.feed_item_id).collect();
        assert!(ids.ends_with(&[second, first]), "{ids:?}");

        let job = jobs.iter().find(|j| j.feed_item_id == first).unwrap();
        assert_eq!(job.input, "title: Title one | text: First summary");
        assert_eq!(job.content_hash, content_hash(&job.input));
    }

    #[sqlx::test]
    async fn claim_respects_limit_and_leases(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        for key in ["a", "b", "c"] {
            item(&pool, feed.id, key, "s").await;
        }
        sqlx::query!("DELETE FROM feed_items WHERE feed_id <> $1", feed.id)
            .execute(&pool)
            .await
            .unwrap();

        let first = claim_items_needing_embedding(&pool, MODEL, 2, 60)
            .await
            .unwrap();
        assert_eq!(first.len(), 2);
        let second = claim_items_needing_embedding(&pool, MODEL, 2, 60)
            .await
            .unwrap();
        assert_eq!(second.len(), 1, "leased items must not be handed out again");
        assert!(
            claim_items_needing_embedding(&pool, MODEL, 2, 60)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[sqlx::test]
    async fn claim_reissues_expired_leases(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;
        sqlx::query!("DELETE FROM feed_items WHERE id <> $1", id)
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            claim_items_needing_embedding(&pool, MODEL, 5, 60)
                .await
                .unwrap()
                .len(),
            1
        );
        sqlx::query!(
            "UPDATE feed_item_embedding_leases SET leased_until = NOW() - INTERVAL '1 second'"
        )
        .execute(&pool)
        .await
        .unwrap();
        assert_eq!(
            claim_items_needing_embedding(&pool, MODEL, 5, 60)
                .await
                .unwrap()
                .len(),
            1
        );
    }

    #[sqlx::test]
    async fn stored_items_are_not_claimed_unless_model_changes(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;
        sqlx::query!("DELETE FROM feed_items WHERE id <> $1", id)
            .execute(&pool)
            .await
            .unwrap();

        claim_items_needing_embedding(&pool, MODEL, 5, 60)
            .await
            .unwrap();
        let stored = store_item_embeddings(&pool, MODEL, &[embedding(id, &[1.0, 0.0])])
            .await
            .unwrap();
        assert_eq!(stored, 1);

        assert!(
            claim_items_needing_embedding(&pool, MODEL, 5, 60)
                .await
                .unwrap()
                .is_empty()
        );
        let other = claim_items_needing_embedding(&pool, "other-model", 5, 60)
            .await
            .unwrap();
        assert_eq!(other.len(), 1);
        assert_eq!(other[0].feed_item_id, id);
    }

    #[sqlx::test]
    async fn store_normalizes_replaces_and_skips_deleted_items(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;

        store_item_embeddings(&pool, "old", &[embedding(id, &[1.0, 0.0])])
            .await
            .unwrap();
        let stored = store_item_embeddings(
            &pool,
            MODEL,
            &[embedding(id, &[3.0, 4.0]), embedding(i64::MAX, &[1.0, 0.0])],
        )
        .await
        .unwrap();
        assert_eq!(stored, 1);

        let row = sqlx::query!(
            "SELECT model, embedding FROM feed_item_embeddings WHERE feed_item_id = $1",
            id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.model, MODEL);
        let embedding = decode_embedding(&row.embedding);
        assert!((embedding[0] - 0.6).abs() < 1e-6);
        assert!((embedding[1] - 0.8).abs() < 1e-6);
    }

    #[sqlx::test]
    async fn store_rejects_degenerate_vectors_without_partial_writes(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;

        let err = store_item_embeddings(
            &pool,
            MODEL,
            &[embedding(id, &[1.0, 0.0]), embedding(id, &[0.0, 0.0])],
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid embedding"));

        let count = sqlx::query_scalar!("SELECT COUNT(*) FROM feed_item_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, Some(0));
    }

    #[sqlx::test]
    async fn embeddings_are_deleted_with_their_item(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;
        store_item_embeddings(&pool, MODEL, &[embedding(id, &[1.0])])
            .await
            .unwrap();

        sqlx::query!("DELETE FROM feed_items WHERE id = $1", id)
            .execute(&pool)
            .await
            .unwrap();
        let count = sqlx::query_scalar!("SELECT COUNT(*) FROM feed_item_embeddings")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, Some(0));
    }

    /// Refresh `index` and score `query`, as a search does.
    async fn search(
        index: &EmbeddingIndex,
        pool: &PgPool,
        query: &[f32],
        feed_id: Option<i64>,
    ) -> Vec<(i64, f32)> {
        index.refresh(pool).await.unwrap();
        index.similarities(query, feed_id).await
    }

    #[sqlx::test]
    async fn index_holds_only_embeddings_of_its_model(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let embedded = item(&pool, feed.id, "embedded", "s").await;
        let other_model = item(&pool, feed.id, "other-model", "s").await;
        let _unembedded = item(&pool, feed.id, "none", "s").await;
        store_item_embeddings(&pool, MODEL, &[embedding(embedded, &[1.0, 0.0])])
            .await
            .unwrap();
        store_item_embeddings(&pool, "other", &[embedding(other_model, &[1.0, 0.0])])
            .await
            .unwrap();

        let index = EmbeddingIndex::new(MODEL);
        let scores = search(&index, &pool, &[1.0, 0.0], None).await;
        let ids: Vec<i64> = scores.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![embedded]);
        assert_eq!(index.item_count().await, 1);
    }

    #[sqlx::test]
    async fn index_dates_prefer_the_publish_date(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let old = item(&pool, feed.id, "old", "s").await;
        let new = item(&pool, feed.id, "new", "s").await;
        sqlx::query!(
            "UPDATE feed_item_details SET published_at = NOW() - INTERVAL '365 days' WHERE feed_item_id = $1",
            old
        )
        .execute(&pool)
        .await
        .unwrap();
        store_item_embeddings(
            &pool,
            MODEL,
            &[embedding(old, &[1.0, 0.0]), embedding(new, &[1.0, 0.0])],
        )
        .await
        .unwrap();

        let index = EmbeddingIndex::new(MODEL);
        index.refresh(&pool).await.unwrap();
        let dates = index.dates([old, new, i64::MAX]).await;
        let age_days = |id: i64| (Utc::now() - dates[&id]).num_days();
        assert_eq!(dates.len(), 2, "unknown ids are skipped");
        // The database and test clocks may differ slightly.
        assert!((364..=365).contains(&age_days(old)), "{}", age_days(old));
        assert!((-1..=0).contains(&age_days(new)), "{}", age_days(new));
    }

    #[sqlx::test]
    async fn semantic_similarities_scores_every_embedded_item(pool: PgPool) {
        let feed_a = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let feed_b = upsert_feed_by_url(&pool, "https://example.com/b.xml")
            .await
            .unwrap();
        let near = item(&pool, feed_a.id, "near", "s").await;
        let far = item(&pool, feed_a.id, "far", "s").await;
        let other_feed = item(&pool, feed_b.id, "other", "s").await;
        store_item_embeddings(
            &pool,
            MODEL,
            &[
                embedding(near, &[1.0, 0.0]),
                embedding(far, &[0.0, 1.0]),
                embedding(other_feed, &[1.0, 0.0]),
            ],
        )
        .await
        .unwrap();

        let mut scores = search(
            &EmbeddingIndex::new(MODEL),
            &pool,
            &[1.0, 0.0],
            Some(feed_a.id),
        )
        .await;
        scores.sort_by_key(|(id, _)| *id);
        assert_eq!(scores.len(), 2, "low scores are kept, other feeds are not");
        assert_eq!(scores[0].0, near);
        assert!((scores[0].1 - 1.0).abs() < 1e-6);
        assert_eq!(scores[1].0, far);
        assert!(scores[1].1.abs() < 1e-6);
    }

    #[test]
    fn embedding_bytes_round_trip_in_float4send_layout() {
        let bytes = encode_embedding(&[1.0, -0.5]);
        assert_eq!(bytes, vec![0x3f, 0x80, 0, 0, 0xbf, 0, 0, 0]);
        assert_eq!(decode_embedding(&bytes), vec![1.0, -0.5]);
    }

    #[sqlx::test]
    async fn index_picks_up_embeddings_stored_after_first_search(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let first = item(&pool, feed.id, "first", "s").await;
        let second = item(&pool, feed.id, "second", "s").await;
        let index = EmbeddingIndex::new(MODEL);

        store_item_embeddings(&pool, MODEL, &[embedding(first, &[1.0, 0.0])])
            .await
            .unwrap();
        let scores = search(&index, &pool, &[1.0, 0.0], None).await;
        assert_eq!(scores.len(), 1);

        store_item_embeddings(
            &pool,
            MODEL,
            &[
                embedding(second, &[1.0, 0.0]),
                embedding(first, &[0.0, 1.0]),
            ],
        )
        .await
        .unwrap();
        let mut scores = search(&index, &pool, &[1.0, 0.0], None).await;
        scores.sort_by_key(|(id, _)| *id);
        assert_eq!(scores.len(), 2, "new embeddings are added once");
        assert!(
            scores[0].1.abs() < 1e-6,
            "updated embeddings replace old ones"
        );
        assert!((scores[1].1 - 1.0).abs() < 1e-6);
    }

    #[sqlx::test]
    async fn reconcile_drops_deleted_items_and_keeps_the_rest(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let a = item(&pool, feed.id, "a", "s").await;
        let deleted = item(&pool, feed.id, "deleted", "s").await;
        let c = item(&pool, feed.id, "c", "s").await;
        store_item_embeddings(
            &pool,
            MODEL,
            &[
                embedding(a, &[1.0, 0.0]),
                embedding(deleted, &[1.0, 0.0]),
                embedding(c, &[0.0, 1.0]),
            ],
        )
        .await
        .unwrap();
        let index = EmbeddingIndex::new(MODEL);
        index.refresh(&pool).await.unwrap();

        sqlx::query!("DELETE FROM feed_items WHERE id = $1", deleted)
            .execute(&pool)
            .await
            .unwrap();
        let stats = index.reconcile(&pool).await.unwrap();
        assert_eq!(
            stats,
            ReconcileStats {
                removed: 1,
                fetched: 0
            }
        );

        let mut scores = search(&index, &pool, &[1.0, 0.0], None).await;
        scores.sort_by_key(|(id, _)| *id);
        assert_eq!(scores.len(), 2);
        assert_eq!(scores[0].0, a);
        assert!((scores[0].1 - 1.0).abs() < 1e-6);
        assert_eq!(scores[1].0, c);
        assert!(scores[1].1.abs() < 1e-6);
    }

    #[sqlx::test]
    async fn reconcile_fetches_rows_missed_by_incremental_refresh(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let seen = item(&pool, feed.id, "seen", "s").await;
        let late = item(&pool, feed.id, "late", "s").await;
        store_item_embeddings(&pool, MODEL, &[embedding(seen, &[1.0, 0.0])])
            .await
            .unwrap();
        let index = EmbeddingIndex::new(MODEL);
        index.refresh(&pool).await.unwrap();

        // A row committed late with an `embedded_at` older than the refresh overlap.
        store_item_embeddings(&pool, MODEL, &[embedding(late, &[1.0, 0.0])])
            .await
            .unwrap();
        sqlx::query!(
            "UPDATE feed_item_embeddings SET embedded_at = NOW() - INTERVAL '1 day' WHERE feed_item_id = $1",
            late
        )
        .execute(&pool)
        .await
        .unwrap();
        index.refresh(&pool).await.unwrap();
        assert_eq!(index.item_count().await, 1, "incremental refresh misses it");

        let stats = index.reconcile(&pool).await.unwrap();
        assert_eq!(
            stats,
            ReconcileStats {
                removed: 0,
                fetched: 1
            }
        );
        assert_eq!(index.item_count().await, 2);
    }

    #[sqlx::test]
    async fn reconcile_updates_recency_dates(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;
        store_item_embeddings(&pool, MODEL, &[embedding(id, &[1.0, 0.0])])
            .await
            .unwrap();
        let index = EmbeddingIndex::new(MODEL);
        index.refresh(&pool).await.unwrap();

        sqlx::query!(
            "UPDATE feed_item_details SET published_at = NOW() - INTERVAL '30 days' WHERE feed_item_id = $1",
            id
        )
        .execute(&pool)
        .await
        .unwrap();
        let stats = index.reconcile(&pool).await.unwrap();
        assert_eq!(
            stats,
            ReconcileStats {
                removed: 0,
                fetched: 0
            }
        );
        let published_at = sqlx::query_scalar!(
            "SELECT published_at FROM feed_item_details WHERE feed_item_id = $1",
            id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(index.dates([id]).await[&id], published_at);
    }

    #[sqlx::test]
    async fn refresh_overlap_recovers_late_embeddings_without_duplicating_seen_items(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/overlap.xml")
            .await
            .unwrap();
        let seen = item(&pool, feed.id, "seen", "s").await;
        let late = item(&pool, feed.id, "late", "s").await;
        let timestamp = Utc::now();
        store_item_embeddings(&pool, MODEL, &[embedding(seen, &[1.0, 0.0])])
            .await
            .unwrap();
        sqlx::query("UPDATE feed_item_embeddings SET embedded_at = $1 WHERE feed_item_id = $2")
            .bind(timestamp)
            .bind(seen)
            .execute(&pool)
            .await
            .unwrap();
        let index = EmbeddingIndex::new(MODEL);
        index.refresh(&pool).await.unwrap();

        store_item_embeddings(&pool, MODEL, &[embedding(late, &[0.0, 1.0])])
            .await
            .unwrap();
        sqlx::query("UPDATE feed_item_embeddings SET embedded_at = $1 WHERE feed_item_id = $2")
            .bind(timestamp - chrono::Duration::minutes(1))
            .bind(late)
            .execute(&pool)
            .await
            .unwrap();
        index.refresh(&pool).await.unwrap();

        let mut scores = index.similarities(&[1.0, 0.0], None).await;
        scores.sort_by_key(|(id, _)| *id);
        assert_eq!(scores, vec![(seen, 1.0), (late, 0.0)]);
        assert_eq!(index.item_count().await, 2);
    }

    #[sqlx::test]
    async fn failed_refresh_and_reconcile_keep_the_previous_index_queryable(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/failure.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "kept", "s").await;
        store_item_embeddings(&pool, MODEL, &[embedding(id, &[1.0, 0.0])])
            .await
            .unwrap();
        let index = EmbeddingIndex::new(MODEL);
        index.refresh(&pool).await.unwrap();
        let dates = index.dates([id]).await;
        pool.close().await;

        assert!(index.refresh(&pool).await.is_err());
        assert!(index.reconcile(&pool).await.is_err());

        assert_eq!(index.similarities(&[1.0, 0.0], None).await, vec![(id, 1.0)]);
        assert_eq!(index.dates([id]).await, dates);
        assert_eq!(index.item_count().await, 1);
    }

    #[sqlx::test]
    async fn read_by_ids_preserves_requested_order(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let a = item(&pool, feed.id, "a", "summary a").await;
        let b = item(&pool, feed.id, "b", "summary b").await;

        let items = read_feed_items_with_detail_by_ids(&pool, &[b, i64::MAX, a])
            .await
            .unwrap();
        let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
        assert_eq!(ids, vec![b, a]);
        assert_eq!(items[0].summary.as_deref(), Some("summary b"));
    }
}
