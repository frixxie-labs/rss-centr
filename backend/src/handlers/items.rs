use axum::{Json, extract::Path, extract::Query, extract::State};
use serde::Deserialize;
use sqlx::PgPool;
use tracing::{instrument, warn};

use crate::feed::feed_item::{FeedItem, FeedItemDetail, FeedItemWithDetail};
use crate::feed::feed_item::{
    read_feed_item, read_feed_item_detail, read_feed_items_by_feed,
    read_latest_feed_items_with_detail,
};

use super::error::HandlerError;

#[utoipa::path(
    get,
    path = "/api/feeds/{feed_id}/items",
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "List of feed items", body = [FeedItem]),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_items_by_feed(
    State(pool): State<PgPool>,
    Path(feed_id): Path<i64>,
) -> Result<Json<Vec<FeedItem>>, HandlerError> {
    let items = read_feed_items_by_feed(&pool, feed_id).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(items))
}

#[derive(Debug, Clone, Deserialize)]
pub struct LatestItemsQuery {
    pub limit: Option<u32>,
    pub feed_id: Option<i64>,
    pub q: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/items/latest",
    params(
        ("limit" = Option<u32>, Query, description = "Maximum number of items to return"),
        ("feed_id" = Option<i64>, Query, description = "Only return items from this feed"),
        ("q" = Option<String>, Query, description = "Search items by title, URL, summary, content, author, or feed")
    ),
    responses(
        (status = 200, description = "List of latest feed items", body = [FeedItemWithDetail]),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_latest_items(
    State(pool): State<PgPool>,
    Query(query): Query<LatestItemsQuery>,
) -> Result<Json<Vec<FeedItemWithDetail>>, HandlerError> {
    let search_query = query.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
    let items = read_latest_feed_items_with_detail(
        &pool,
        query.limit.map(i64::from),
        query.feed_id,
        search_query,
    )
    .await
    .map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(items))
}

#[utoipa::path(
    get,
    path = "/api/items/{item_id}",
    params(
        ("item_id" = i64, Path, description = "Feed item ID")
    ),
    responses(
        (status = 200, description = "Feed item", body = FeedItem),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_item_by_id(
    State(pool): State<PgPool>,
    Path(item_id): Path<i64>,
) -> Result<Json<FeedItem>, HandlerError> {
    let item = read_feed_item(&pool, item_id).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(item))
}

#[utoipa::path(
    get,
    path = "/api/items/{item_id}/detail",
    params(
        ("item_id" = i64, Path, description = "Feed item ID")
    ),
    responses(
        (status = 200, description = "Feed item detail", body = FeedItemDetail),
        (status = 500, description = "Internal server error"),
    ),
    tag = "items"
)]
#[instrument]
pub async fn fetch_item_detail(
    State(pool): State<PgPool>,
    Path(item_id): Path<i64>,
) -> Result<Json<FeedItemDetail>, HandlerError> {
    let detail = read_feed_item_detail(&pool, item_id).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(detail))
}
