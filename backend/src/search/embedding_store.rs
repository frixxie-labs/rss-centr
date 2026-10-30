use std::collections::BTreeSet;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use pgvector::Vector;
use rss_centr_core::semantic::{
    EmbeddingJob, ItemEmbedding, content_hash, embedding_input, normalize,
};
use sqlx::PgPool;

use super::FeedCentroid;

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
/// Ensures the model has a nearest-neighbour index for each stored dimension.
/// Returns the number of embeddings stored.
pub async fn store_item_embeddings(
    pool: &PgPool,
    model: &str,
    embeddings: &[ItemEmbedding],
) -> Result<u64> {
    // Store everything in one statement: the database is a network hop away,
    // so per-item round-trips dominate the cost.
    let vectors = embeddings
        .iter()
        .map(|item| {
            normalize(&item.embedding).map(Vector::from).ok_or_else(|| {
                anyhow!(
                    "invalid embedding for feed_item_id={}: empty or degenerate vector",
                    item.feed_item_id
                )
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let ids: Vec<i64> = embeddings.iter().map(|item| item.feed_item_id).collect();
    let hashes: Vec<String> = embeddings
        .iter()
        .map(|item| item.content_hash.clone())
        .collect();
    let dimensions: BTreeSet<i32> = vectors
        .iter()
        .map(|vector| vector.as_slice().len() as i32)
        .collect();

    let stored = sqlx::query_scalar!(
        r#"
        WITH input AS (
            SELECT DISTINCT ON (feed_item_id) *
            FROM UNNEST($2::BIGINT[], $3::TEXT[], $4::vector[]) WITH ORDINALITY
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
        &vectors as &[Vector],
    )
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to store {} embeddings", ids.len()))?;

    for dimension in dimensions {
        sqlx::query!(
            "SELECT ensure_feed_item_embedding_index($1, $2)",
            model,
            dimension
        )
        .execute(pool)
        .await
        .with_context(|| {
            format!("failed to create embedding index for model={model} dimension={dimension}")
        })?;
    }
    Ok(stored as u64)
}

/// A search candidate and its cosine similarity to the query.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScoredEmbedding {
    pub(crate) id: i64,
    pub(crate) similarity: f32,
    /// Effective publication date, see [`rss_centr_core::semantic::item_date`].
    pub(crate) date: DateTime<Utc>,
}

/// The `limit` items of `model` most similar to the unit vector `query`
/// (approximate once the model has an HNSW index), optionally within one
/// feed, plus the items in `include` that have an embedding.
pub(crate) async fn nearest_embeddings(
    pool: &PgPool,
    model: &str,
    query: &[f32],
    feed_id: Option<i64>,
    limit: i64,
    include: &[i64],
) -> Result<Vec<ScoredEmbedding>> {
    let limit = i32::try_from(limit).unwrap_or(i32::MAX);
    sqlx::query_as!(
        ScoredEmbedding,
        r#"
        SELECT id as "id!", similarity as "similarity!", date as "date!"
        FROM nearest_feed_item_embeddings($1, $2::vector, $3, $4, $5)
        "#,
        model,
        Vector::from(query.to_vec()) as Vector,
        feed_id,
        limit,
        include,
    )
    .fetch_all(pool)
    .await
    .with_context(|| {
        format!("failed to search embeddings for model={model} feed_id={feed_id:?} limit={limit}")
    })
}

/// Sum and count the embeddings of `model` per feed and dimension.
pub(crate) async fn read_feed_centroids(pool: &PgPool, model: &str) -> Result<Vec<FeedCentroid>> {
    sqlx::query_as!(
        FeedCentroid,
        r#"
        SELECT f.feed_id as "feed_id!",
               SUM(e.embedding)::REAL[] as "sum!",
               COUNT(*) as "count!"
        FROM feed_item_embeddings e
        INNER JOIN feed_items f ON f.id = e.feed_item_id
        WHERE e.model = $1
        GROUP BY f.feed_id, vector_dims(e.embedding)
        "#,
        model,
    )
    .fetch_all(pool)
    .await
    .with_context(|| format!("failed to read embedding centroids for model={model}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_item::insert_feed_item;
    use crate::feed::feed_item_detail::insert_feed_item_detail;
    use crate::feed::feed_subscription::upsert_feed_by_url;

    pub(super) const MODEL: &str = "test-model";

    pub(super) async fn item(pool: &PgPool, feed_id: i64, key: &str, summary: &str) -> i64 {
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

    pub(super) fn embedding(feed_item_id: i64, vector: &[f32]) -> ItemEmbedding {
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
            r#"SELECT model, embedding::REAL[] as "embedding!" FROM feed_item_embeddings WHERE feed_item_id = $1"#,
            id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(row.model, MODEL);
        let embedding = row.embedding;
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
    async fn nearest_returns_the_most_similar_items_of_the_model_and_feed(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let other_feed = upsert_feed_by_url(&pool, "https://example.com/b.xml")
            .await
            .unwrap();
        let close = item(&pool, feed.id, "close", "s").await;
        let middle = item(&pool, feed.id, "middle", "s").await;
        let far = item(&pool, feed.id, "far", "s").await;
        let elsewhere = item(&pool, other_feed.id, "elsewhere", "s").await;
        let other_model = item(&pool, feed.id, "other-model", "s").await;
        let other_dimension = item(&pool, feed.id, "other-dimension", "s").await;
        store_item_embeddings(
            &pool,
            MODEL,
            &[
                embedding(close, &[1.0, 0.1]),
                embedding(middle, &[1.0, 1.0]),
                embedding(far, &[0.0, 1.0]),
                embedding(elsewhere, &[1.0, 0.0]),
                embedding(other_dimension, &[1.0, 0.0, 0.0]),
            ],
        )
        .await
        .unwrap();
        store_item_embeddings(&pool, "other", &[embedding(other_model, &[1.0, 0.0])])
            .await
            .unwrap();

        let all = nearest_embeddings(&pool, MODEL, &[1.0, 0.0], None, 10, &[])
            .await
            .unwrap();
        let mut ids: Vec<i64> = all.iter().map(|e| e.id).collect();
        ids.sort();
        assert_eq!(ids, vec![close, middle, far, elsewhere]);
        let middle_similarity = all.iter().find(|e| e.id == middle).unwrap().similarity;
        assert!((middle_similarity - 0.5f32.sqrt()).abs() < 1e-5);

        let mut nearest = nearest_embeddings(&pool, MODEL, &[1.0, 0.0], Some(feed.id), 2, &[])
            .await
            .unwrap();
        nearest.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
        assert_eq!(
            nearest.iter().map(|e| e.id).collect::<Vec<_>>(),
            vec![close, middle]
        );

        let included = nearest_embeddings(
            &pool,
            MODEL,
            &[1.0, 0.0],
            Some(feed.id),
            1,
            &[far, other_model, i64::MAX],
        )
        .await
        .unwrap();
        let mut ids: Vec<i64> = included.iter().map(|e| e.id).collect();
        ids.sort();
        assert_eq!(ids, vec![close, far], "included items need an embedding");
    }

    #[sqlx::test]
    async fn nearest_returns_the_effective_publication_date(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let id = item(&pool, feed.id, "a", "s").await;
        let published = DateTime::from_timestamp(1_600_000_000, 0).unwrap();
        sqlx::query!(
            "UPDATE feed_item_details SET published_at = $1 WHERE feed_item_id = $2",
            published,
            id
        )
        .execute(&pool)
        .await
        .unwrap();
        store_item_embeddings(&pool, MODEL, &[embedding(id, &[1.0, 0.0])])
            .await
            .unwrap();

        let nearest = nearest_embeddings(&pool, MODEL, &[1.0, 0.0], None, 10, &[])
            .await
            .unwrap();
        assert_eq!(nearest[0].date, published);
    }

    #[sqlx::test]
    async fn store_creates_one_hnsw_index_per_model_and_dimension(pool: PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/a.xml")
            .await
            .unwrap();
        let a = item(&pool, feed.id, "a", "s").await;
        let b = item(&pool, feed.id, "b", "s").await;
        let c = item(&pool, feed.id, "c", "s").await;
        store_item_embeddings(&pool, MODEL, &[embedding(a, &[1.0, 0.0])])
            .await
            .unwrap();
        store_item_embeddings(
            &pool,
            MODEL,
            &[embedding(b, &[0.0, 1.0]), embedding(c, &[1.0, 0.0, 0.0])],
        )
        .await
        .unwrap();
        store_item_embeddings(&pool, "other", &[embedding(c, &[1.0, 0.0, 0.0])])
            .await
            .unwrap();

        let mut indexes = sqlx::query_scalar!(
            r#"
            SELECT indexdef as "indexdef!" FROM pg_indexes
            WHERE tablename = 'feed_item_embeddings' AND indexdef LIKE '%USING hnsw%'
            "#
        )
        .fetch_all(&pool)
        .await
        .unwrap();
        indexes.sort();
        assert_eq!(indexes.len(), 3, "{indexes:#?}");
        assert!(indexes.iter().any(|def| def.contains("vector(2)")
            && def.contains(&format!("model = '{MODEL}'"))));
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
}
