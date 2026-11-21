use axum::{Json, extract::Path, extract::State};
use sqlx::PgPool;
use tracing::instrument;

use crate::feed::feed_item::{FeedItem, read_feed_item, read_feed_items_by_feed};
use crate::feed::feed_item_detail::{FeedItemDetail, read_feed_item_detail};

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
    let items = read_feed_items_by_feed(&pool, feed_id)
        .await
        .map_err(|e| HandlerError::from_db(e, "Failed to fetch data from database"))?;
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
    let item = read_feed_item(&pool, item_id)
        .await
        .map_err(|e| HandlerError::from_db(e, "Failed to fetch data from database"))?;
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
    let detail = read_feed_item_detail(&pool, item_id)
        .await
        .map_err(|e| HandlerError::from_db(e, "Failed to fetch data from database"))?;
    Ok(Json(detail))
}
