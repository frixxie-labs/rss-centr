use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::prelude::FromRow;
use sqlx::{PgPool, Postgres};
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, FromRow, ToSchema)]
pub struct FeedItemDetail {
    pub id: i64,
    pub feed_item_id: i64,
    pub summary: String,
    pub content: String,
    pub author: String,
    pub published_at: DateTime<Utc>,
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
        return Err(crate::feed::NotFound::new(format!(
            "no feed item detail found for feed_item_id={feed_item_id}"
        ))
        .into());
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
        return Err(crate::feed::NotFound::new(format!(
            "no feed item detail found for feed_item_id={feed_item_id}"
        ))
        .into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_item::{delete_feed_item, insert_feed_item};
    use crate::feed::feed_subscription::upsert_feed_by_url;

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
