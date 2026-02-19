use axum::{
    Json,
    extract::{Path, State},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::mpsc::Sender;
use tracing::{instrument, warn};
use utoipa::ToSchema;

use crate::{
    background_tasks::IngestJob,
    feed::feed_subscription::{self, FeedSubscription},
};

use super::error::HandlerError;

type FeedState = State<(SqlitePool, Sender<IngestJob>)>;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct NewFeed {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UpdateFeedEnabled {
    pub is_enabled: bool,
}

#[utoipa::path(
    get,
    path = "/api/feeds",
    responses(
        (status = 200, description = "List of feeds", body = [FeedSubscription]),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn fetch_feeds(
    State(app_state): FeedState,
) -> Result<Json<Vec<FeedSubscription>>, HandlerError> {
    let (pool, _tx) = app_state;
    let feeds = feed_subscription::list_feeds(&pool).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to fetch data from database")
    })?;
    Ok(Json(feeds))
}

#[utoipa::path(
    post,
    path = "/api/feeds",
    request_body = NewFeed,
    responses(
        (status = 200, description = "Feed upserted", body = FeedSubscription),
        (status = 400, description = "Invalid input"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn create_feed(
    State(app_state): FeedState,
    Json(body): Json<NewFeed>,
) -> Result<Json<FeedSubscription>, HandlerError> {
    let (pool, _tx) = app_state;
    if body.url.trim().is_empty() {
        return Err(HandlerError::bad_request("Invalid input"));
    }

    let feed = feed_subscription::upsert_feed_by_url(&pool, body.url.trim())
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to store data in database")
        })?;
    Ok(Json(feed))
}

#[utoipa::path(
    get,
    path = "/api/feeds/{feed_id}",
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "Feed found", body = FeedSubscription),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn fetch_feed_by_id(
    State(app_state): FeedState,
    Path(feed_id): Path<i64>,
) -> Result<Json<FeedSubscription>, HandlerError> {
    let (pool, _tx) = app_state;
    let feed = feed_subscription::read_feed(&pool, feed_id)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to fetch data from database")
        })?;
    Ok(Json(feed))
}

#[utoipa::path(
    put,
    path = "/api/feeds/{feed_id}",
    request_body = UpdateFeedEnabled,
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "Feed updated", body = String),
        (status = 400, description = "Invalid input"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn update_feed_enabled(
    State(app_state): FeedState,
    Path(feed_id): Path<i64>,
    Json(body): Json<UpdateFeedEnabled>,
) -> Result<String, HandlerError> {
    let (pool, _tx) = app_state;
    feed_subscription::set_feed_enabled(&pool, feed_id, body.is_enabled)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to update feed")
        })?;
    Ok("OK".to_string())
}

#[utoipa::path(
    post,
    path = "/api/feeds/{feed_id}/ingest",
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 202, description = "Ingest queued", body = String),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn queue_ingest_feed(
    State(app_state): FeedState,
    Path(feed_id): Path<i64>,
) -> Result<Response, HandlerError>
where
    Response: IntoResponse,
{
    let (_pool, tx) = app_state;

    tx.send(IngestJob::FeedId(feed_id)).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::internal(format!(
            "Failed to send ingest job to background thread: {e}"
        ))
    })?;

    let resp = Response::builder()
        .status(202)
        .body("queued".into())
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::internal(format!("Failed to build response: {e}"))
        })?;
    Ok(resp)
}

#[utoipa::path(
    delete,
    path = "/api/feeds/{feed_id}",
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "Feed deleted", body = String),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn delete_feed(
    State(app_state): FeedState,
    Path(feed_id): Path<i64>,
) -> Result<String, HandlerError> {
    let (pool, _tx) = app_state;
    feed_subscription::delete_feed(&pool, feed_id)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to delete feed")
        })?;
    Ok("OK".to_string())
}
