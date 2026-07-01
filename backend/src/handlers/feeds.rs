use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{instrument, warn};
use utoipa::ToSchema;

use crate::{
    feed::feed_subscription::{self, FeedSubscription},
    feed::feed_update_queue,
};

use super::error::HandlerError;

type FeedState = State<PgPool>;

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
    let pool = app_state;
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
    let pool = app_state;
    if body.url.trim().is_empty() {
        return Err(HandlerError::bad_request("Invalid input"));
    }

    let feed = feed_subscription::upsert_feed_by_url(&pool, body.url.trim())
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to store data in database")
        })?;
    feed_update_queue::enqueue_feed_now(&pool, feed.id)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to enqueue feed update")
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
    let pool = app_state;
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
    let pool = app_state;
    feed_subscription::set_feed_enabled(&pool, feed_id, body.is_enabled)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to update feed")
        })?;

    if body.is_enabled {
        feed_update_queue::enqueue_feed_now(&pool, feed_id)
            .await
            .map_err(|e| {
                warn!("failed with error: {e:#}");
                HandlerError::from_db(e, "Failed to enqueue feed update")
            })?;
    }

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
        (status = 409, description = "Feed is paused"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn queue_ingest_feed(
    State(app_state): FeedState,
    Path(feed_id): Path<i64>,
) -> Result<(StatusCode, &'static str), HandlerError> {
    let pool = app_state;

    feed_update_queue::enqueue_feed_now(&pool, feed_id)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            if feed_update_queue::is_feed_paused(&e) {
                return HandlerError::conflict("feed is paused; enable it before fetching now");
            }
            HandlerError::from_db(e, "Failed to enqueue feed update")
        })?;

    Ok((StatusCode::ACCEPTED, "queued"))
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
    let pool = app_state;
    feed_subscription::delete_feed(&pool, feed_id)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to delete feed")
        })?;
    Ok("OK".to_string())
}
