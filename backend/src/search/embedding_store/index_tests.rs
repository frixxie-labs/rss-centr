use super::tests::{MODEL, embedding, item};
use super::*;
use crate::feed::feed_subscription::upsert_feed_by_url;
use crate::search::{EmbeddingIndex, ReconcileStats};

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
