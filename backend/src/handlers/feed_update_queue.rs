use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    routing::post,
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

/// Maximum completion body size. Feeds that include full article content can
/// exceed axum's 2 MB default in a single update.
pub const FEED_UPDATE_BODY_LIMIT: usize = 32 * 1024 * 1024;

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/feed-update-queue/dequeue", post(dequeue_feed_updates))
        .route(
            "/feed-update-queue/{feed_id}/complete",
            post(complete_feed_update_handler),
        )
        .route(
            "/feed-update-queue/{feed_id}/failed",
            post(fail_feed_update_handler),
        )
        .layer(DefaultBodyLimit::max(FEED_UPDATE_BODY_LIMIT))
        .with_state(pool)
}
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
#[instrument(skip(pool, body), fields(items = body.items.len()))]
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::feed_subscription::upsert_feed_by_url;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use rss_centr_core::feed_update_queue::FeedUpdateItemInput;
    use tower::ServiceExt;

    /// A completion request whose JSON body is roughly `bytes` long, made of
    /// full-content items like those in the GitHub Blog feed.
    fn large_completion(bytes: usize) -> Vec<u8> {
        let content = "x".repeat(100_000);
        let items = (0..bytes / content.len())
            .map(|i| FeedUpdateItemInput {
                external_id: format!("item-{i}"),
                title: format!("Post {i}"),
                url: format!("https://example.com/{i}"),
                summary: None,
                content: Some(content.clone()),
                author: None,
                published_at: None,
            })
            .collect();
        serde_json::to_vec(&CompleteFeedUpdateRequest {
            lease_token: "not-the-current-lease".to_string(),
            fetched: true,
            title: None,
            site_url: None,
            etag: None,
            last_modified: None,
            items,
        })
        .unwrap()
    }

    async fn post_completion(pool: PgPool, body: Vec<u8>) -> StatusCode {
        let feed = upsert_feed_by_url(&pool, "https://example.com/large.xml")
            .await
            .unwrap();
        router(pool)
            .oneshot(
                Request::post(format!("/feed-update-queue/{}/complete", feed.id))
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
    }

    #[sqlx::test]
    async fn completion_accepts_bodies_larger_than_the_axum_default(pool: PgPool) {
        // Axum rejects bodies over 2 MB by default; full-content feeds exceed that.
        let status = post_completion(pool, large_completion(5 * 1024 * 1024)).await;
        assert_ne!(status, StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(
            status,
            StatusCode::CONFLICT,
            "body is parsed and lease checked"
        );
    }

    #[sqlx::test]
    async fn completion_still_rejects_bodies_over_the_limit(pool: PgPool) {
        let status =
            post_completion(pool, large_completion(FEED_UPDATE_BODY_LIMIT + 1024 * 1024)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }
}
