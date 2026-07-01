use axum::{
    Json,
    extract::{Path, Query, State},
};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult,
};
use serde::Deserialize;
use sqlx::PgPool;
use tracing::{instrument, warn};
use utoipa::ToSchema;

use crate::feed::feed_update_queue::{
    complete_feed_update, dequeue_due_feeds, fail_feed_update, is_lease_conflict,
};

use super::error::HandlerError;

const DEFAULT_DEQUEUE_LIMIT: i64 = 25;
const MAX_DEQUEUE_LIMIT: i64 = 100;
const DEFAULT_LEASE_SECONDS: i64 = 300;
const MAX_LEASE_SECONDS: i64 = 3600;

#[derive(Debug, Deserialize, ToSchema)]
pub struct DequeueFeedUpdatesQuery {
    pub limit: Option<i64>,
    pub lease_seconds: Option<i64>,
}

#[utoipa::path(
    post,
    path = "/internal/feed-update-queue/dequeue",
    params(
        ("limit" = Option<i64>, Query, description = "Maximum number of feed updates to lease"),
        ("lease_seconds" = Option<i64>, Query, description = "Lease duration in seconds")
    ),
    responses(
        (status = 200, description = "Dequeued feed updates", body = [DequeuedFeedUpdate]),
        (status = 400, description = "Invalid input"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feed update queue"
)]
#[instrument]
pub async fn dequeue_feed_updates(
    State(pool): State<PgPool>,
    Query(query): Query<DequeueFeedUpdatesQuery>,
) -> Result<Json<Vec<DequeuedFeedUpdate>>, HandlerError> {
    let limit = query.limit.unwrap_or(DEFAULT_DEQUEUE_LIMIT);
    let lease_seconds = query.lease_seconds.unwrap_or(DEFAULT_LEASE_SECONDS);
    if !(1..=MAX_DEQUEUE_LIMIT).contains(&limit) {
        return Err(HandlerError::bad_request(format!(
            "limit must be between 1 and {MAX_DEQUEUE_LIMIT}"
        )));
    }
    if !(1..=MAX_LEASE_SECONDS).contains(&lease_seconds) {
        return Err(HandlerError::bad_request(format!(
            "lease_seconds must be between 1 and {MAX_LEASE_SECONDS}"
        )));
    }

    let feeds = dequeue_due_feeds(&pool, limit, lease_seconds)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to dequeue feed updates")
        })?;

    Ok(Json(feeds))
}

#[utoipa::path(
    post,
    path = "/internal/feed-update-queue/{feed_id}/complete",
    request_body = CompleteFeedUpdateRequest,
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "Feed update completed", body = CompleteFeedUpdateResult),
        (status = 409, description = "Lease token is stale or expired"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feed update queue"
)]
#[instrument]
pub async fn complete_feed_update_handler(
    State(pool): State<PgPool>,
    Path(feed_id): Path<i64>,
    Json(body): Json<CompleteFeedUpdateRequest>,
) -> Result<Json<CompleteFeedUpdateResult>, HandlerError> {
    let result = complete_feed_update(&pool, feed_id, body)
        .await
        .map_err(map_queue_error)?;

    Ok(Json(result))
}

#[utoipa::path(
    post,
    path = "/internal/feed-update-queue/{feed_id}/failed",
    request_body = FailedFeedUpdateRequest,
    params(
        ("feed_id" = i64, Path, description = "Feed ID")
    ),
    responses(
        (status = 200, description = "Feed update failure recorded", body = FailedFeedUpdateResult),
        (status = 409, description = "Lease token is stale or expired"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feed update queue"
)]
#[instrument]
pub async fn fail_feed_update_handler(
    State(pool): State<PgPool>,
    Path(feed_id): Path<i64>,
    Json(body): Json<FailedFeedUpdateRequest>,
) -> Result<Json<FailedFeedUpdateResult>, HandlerError> {
    let result = fail_feed_update(&pool, feed_id, body)
        .await
        .map_err(map_queue_error)?;

    Ok(Json(result))
}

fn map_queue_error(err: anyhow::Error) -> HandlerError {
    warn!("failed with error: {err:#}");
    if is_lease_conflict(&err) {
        return HandlerError::conflict(err.to_string());
    }

    HandlerError::from_db(err, "Failed to update feed queue")
}
