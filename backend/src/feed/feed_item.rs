use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use sqlx::prelude::FromRow;
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
pub struct FeedItemDetail {
    pub id: i64,
    pub feed_item_id: i64,
    pub summary: String,
    pub content: String,
    pub author: String,
    pub published_at: DateTime<Utc>,
}

pub async fn insert_feed_item(
    pool: &SqlitePool,
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

pub async fn insert_feed_item_dedup(
    pool: &SqlitePool,
    feed_id: i64,
    external_id: &str,
    title: &str,
    url: &str,
) -> Result<Option<FeedItem>> {
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
    .fetch_optional(pool)
    .await
    .with_context(|| {
        format!(
            "failed to insert (dedup) feed item with feed_id={feed_id} external_id={external_id}"
        )
    })?;

    Ok(row)
}

pub async fn read_feed_item(pool: &SqlitePool, id: i64) -> Result<FeedItem> {
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
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to read feed item with id={id}"))?;

    Ok(row)
}

pub async fn read_feed_items_by_feed(pool: &SqlitePool, feed_id: i64) -> Result<Vec<FeedItem>> {
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

pub async fn read_latest_feed_items(pool: &SqlitePool, limit: i64) -> Result<Vec<FeedItem>> {
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

pub async fn update_feed_item(
    pool: &SqlitePool,
    id: i64,
    title: &str,
    url: &str,
) -> Result<FeedItem> {
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

pub async fn delete_feed_item(pool: &SqlitePool, id: i64) -> Result<()> {
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
    pool: &SqlitePool,
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

pub async fn insert_feed_item_detail_dedup(
    pool: &SqlitePool,
    feed_item_id: i64,
    summary: &str,
    content: &str,
    author: &str,
    published_at: DateTime<Utc>,
) -> Result<Option<FeedItemDetail>> {
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
    .fetch_optional(pool)
    .await
    .with_context(|| {
        format!("failed to insert (dedup) feed item detail for feed_item_id={feed_item_id}")
    })?;

    Ok(row)
}

pub async fn read_feed_item_detail(pool: &SqlitePool, feed_item_id: i64) -> Result<FeedItemDetail> {
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
    .fetch_one(pool)
    .await
    .with_context(|| format!("failed to read feed item detail for feed_item_id={feed_item_id}"))?;

    Ok(row)
}

pub async fn update_feed_item_detail(
    pool: &SqlitePool,
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

pub async fn delete_feed_item_detail(pool: &SqlitePool, feed_item_id: i64) -> Result<()> {
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
    use crate::feed::feed_subscription::upsert_feed_by_url;

    // -----------------------------------------------------------------------
    // FeedItem tests
    // -----------------------------------------------------------------------

    #[sqlx::test]
    async fn test_insert_feed_item(pool: SqlitePool) {
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
    async fn test_insert_feed_item_dedup(pool: SqlitePool) {
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
    async fn test_read_feed_item(pool: SqlitePool) {
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
    async fn test_read_feed_item_not_found(pool: SqlitePool) {
        let result = read_feed_item(&pool, 999).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_read_feed_items_by_feed(pool: SqlitePool) {
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
    async fn test_read_feed_items_by_feed_empty(pool: SqlitePool) {
        let items = read_feed_items_by_feed(&pool, 42).await.unwrap();
        assert!(items.is_empty());
    }

    #[sqlx::test]
    async fn test_read_latest_feed_items_limit_and_order(pool: SqlitePool) {
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
    async fn test_update_feed_item(pool: SqlitePool) {
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
    async fn test_delete_feed_item(pool: SqlitePool) {
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
    async fn test_delete_feed_item_not_found(pool: SqlitePool) {
        let result = delete_feed_item(&pool, 999).await;
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // FeedItemDetail tests
    // -----------------------------------------------------------------------

    #[sqlx::test]
    async fn test_insert_feed_item_detail(pool: SqlitePool) {
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
    async fn test_insert_feed_item_detail_dedup(pool: SqlitePool) {
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
    async fn test_read_feed_item_detail(pool: SqlitePool) {
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
    async fn test_read_feed_item_detail_not_found(pool: SqlitePool) {
        let result = read_feed_item_detail(&pool, 999).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_update_feed_item_detail(pool: SqlitePool) {
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
    async fn test_delete_feed_item_detail(pool: SqlitePool) {
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
    async fn test_delete_feed_item_detail_not_found(pool: SqlitePool) {
        let result = delete_feed_item_detail(&pool, 999).await;
        assert!(result.is_err());
    }

    #[sqlx::test]
    async fn test_cascade_delete_removes_detail(pool: SqlitePool) {
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
