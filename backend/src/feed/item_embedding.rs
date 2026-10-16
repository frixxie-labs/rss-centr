use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use rss_centr_core::semantic::{
    EmbeddingJob, ItemEmbedding, content_hash, dot, embedding_input, normalize, similarity_floor,
};
use sqlx::PgPool;

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
    let mut normalized = Vec::with_capacity(embeddings.len());
    for item in embeddings {
        let Some(vector) = normalize(&item.embedding) else {
            bail!(
                "invalid embedding for feed_item_id={}: empty or degenerate vector",
                item.feed_item_id
            );
        };
        normalized.push((item, vector));
    }

    let mut tx = pool
        .begin()
        .await
        .context("failed to begin embedding transaction")?;
    let mut stored = 0;
    for (item, vector) in normalized {
        let result = sqlx::query!(
            r#"
            INSERT INTO feed_item_embeddings (feed_item_id, model, content_hash, embedding, embedded_at)
            SELECT id, $2, $3, $4, NOW() FROM feed_items WHERE id = $1
            ON CONFLICT (feed_item_id) DO UPDATE
                SET model = excluded.model,
                    content_hash = excluded.content_hash,
                    embedding = excluded.embedding,
                    embedded_at = excluded.embedded_at
            "#,
            item.feed_item_id,
            model,
            item.content_hash,
            &vector,
        )
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "failed to store embedding for feed_item_id={}",
                item.feed_item_id
            )
        })?;
        stored += result.rows_affected();

        sqlx::query!(
            "DELETE FROM feed_item_embedding_leases WHERE feed_item_id = $1",
            item.feed_item_id
        )
        .execute(&mut *tx)
        .await
        .with_context(|| {
            format!(
                "failed to release embedding lease for feed_item_id={}",
                item.feed_item_id
            )
        })?;
    }
    tx.commit()
        .await
        .context("failed to commit embedding transaction")?;
    Ok(stored)
}

/// Rank item ids by cosine similarity to `query` among embeddings for
/// `model`, dropping matches below [`similarity_floor`].
pub async fn semantic_search_ids(
    pool: &PgPool,
    model: &str,
    query: &[f32],
    feed_id: Option<i64>,
    limit: usize,
    min_similarity: f32,
) -> Result<Vec<i64>> {
    let mut scores = semantic_similarities(pool, model, query, feed_id).await?;
    let floor = similarity_floor(&scores, min_similarity);
    scores.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    Ok(scores
        .into_iter()
        .filter(|(_, score)| *score >= floor)
        .take(limit)
        .map(|(id, _)| id)
        .collect())
}

/// Cosine similarity of `query` to every item embedded with `model`, in no
/// particular order. Items with a different vector dimension are skipped.
pub async fn semantic_similarities(
    pool: &PgPool,
    model: &str,
    query: &[f32],
    feed_id: Option<i64>,
) -> Result<Vec<(i64, f32)>> {
    let rows = sqlx::query!(
        r#"
        SELECT e.feed_item_id as "id!: i64", e.embedding as "embedding!: Vec<f32>"
        FROM feed_item_embeddings e
        INNER JOIN feed_items f ON f.id = e.feed_item_id
        WHERE e.model = $1
          AND ($2::BIGINT IS NULL OR f.feed_id = $2)
        "#,
        model,
        feed_id,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read embeddings for model={model} feed_id={feed_id:?}"))?;

    Ok(rows
        .iter()
        .filter(|row| row.embedding.len() == query.len())
        .map(|row| (row.id, dot(query, &row.embedding)))
        .filter(|(_, score)| score.is_finite())
        .collect())
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
    use chrono::Utc;

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
            r#"SELECT model, embedding as "embedding!: Vec<f32>" FROM feed_item_embeddings WHERE feed_item_id = $1"#,
            id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.model, MODEL);
        assert!((row.embedding[0] - 0.6).abs() < 1e-6);
        assert!((row.embedding[1] - 0.8).abs() < 1e-6);
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

    #[sqlx::test]
    async fn semantic_search_ranks_by_similarity_and_filters(pool: PgPool) {
        let feed_a = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let feed_b = upsert_feed_by_url(&pool, "https://example.com/b.xml")
            .await
            .unwrap();
        let near = item(&pool, feed_a.id, "near", "s").await;
        let far = item(&pool, feed_a.id, "far", "s").await;
        let other_feed = item(&pool, feed_b.id, "other", "s").await;
        let other_model = item(&pool, feed_a.id, "other-model", "s").await;
        let _unembedded = item(&pool, feed_a.id, "none", "s").await;

        store_item_embeddings(
            &pool,
            MODEL,
            &[
                embedding(near, &[0.9, 0.1]),
                embedding(far, &[0.0, 1.0]),
                embedding(other_feed, &[1.0, 0.0]),
            ],
        )
        .await
        .unwrap();
        store_item_embeddings(&pool, "other", &[embedding(other_model, &[1.0, 0.0])])
            .await
            .unwrap();

        let ids = semantic_search_ids(&pool, MODEL, &[1.0, 0.0], None, 10, -1.0)
            .await
            .unwrap();
        assert_eq!(
            ids,
            vec![other_feed, near],
            "far scores 0, below 60% of the best match"
        );

        let ids = semantic_search_ids(&pool, MODEL, &[1.0, 0.0], Some(feed_a.id), 1, -1.0)
            .await
            .unwrap();
        assert_eq!(ids, vec![near]);

        let ids = semantic_search_ids(&pool, MODEL, &[1.0, 0.0], None, 10, 0.5)
            .await
            .unwrap();
        assert_eq!(ids, vec![other_feed, near], "unrelated items are dropped");
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

        let mut scores = semantic_similarities(&pool, MODEL, &[1.0, 0.0], Some(feed_a.id))
            .await
            .unwrap();
        scores.sort_by_key(|(id, _)| *id);
        assert_eq!(scores.len(), 2, "low scores are kept, other feeds are not");
        assert_eq!(scores[0].0, near);
        assert!((scores[0].1 - 1.0).abs() < 1e-6);
        assert_eq!(scores[1].0, far);
        assert!(scores[1].1.abs() < 1e-6);
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
