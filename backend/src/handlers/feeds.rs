use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::instrument;
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
    let feeds = feed_subscription::list_feeds(&pool)
        .await
        .map_err(|e| HandlerError::from_db(e, "Failed to fetch data from database"))?;
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
        .map_err(|e| HandlerError::from_db(e, "Failed to store data in database"))?;
    // Re-adding a paused feed must not silently resume it.
    if feed.is_enabled {
        feed_update_queue::enqueue_feed_now(&pool, feed.id)
            .await
            .map_err(|e| HandlerError::from_feed_queue(e, "Failed to enqueue feed update"))?;
    }
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
        .map_err(|e| HandlerError::from_db(e, "Failed to fetch data from database"))?;
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
        (status = 404, description = "Feed not found"),
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
    let db_error = |e: anyhow::Error| HandlerError::from_db(e, "Failed to update feed");
    let mut tx = pool.begin().await.map_err(|e| db_error(e.into()))?;
    feed_subscription::set_feed_enabled(&mut *tx, feed_id, body.is_enabled)
        .await
        .map_err(db_error)?;

    if body.is_enabled {
        feed_update_queue::enqueue_feed_now(&mut *tx, feed_id)
            .await
            .map_err(|e| HandlerError::from_feed_queue(e, "Failed to enqueue feed update"))?;
    }

    tx.commit().await.map_err(|e| db_error(e.into()))?;
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
        .map_err(|e| HandlerError::from_feed_queue(e, "Failed to enqueue feed update"))?;

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
        .map_err(|e| HandlerError::from_db(e, "Failed to delete feed"))?;
    Ok("OK".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[sqlx::test]
    async fn manual_ingest_preserves_accepted_paused_and_missing_feed_responses(pool: PgPool) {
        let feed = feed_subscription::upsert_feed_by_url(&pool, "https://example.com/ingest.xml")
            .await
            .unwrap();
        let (status, body) = queue_ingest_feed(State(pool.clone()), Path(feed.id))
            .await
            .unwrap();
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body, "queued");

        feed_subscription::set_feed_enabled(&pool, feed.id, false)
            .await
            .unwrap();
        let paused = queue_ingest_feed(State(pool.clone()), Path(feed.id))
            .await
            .unwrap_err();
        assert_eq!(paused.status, StatusCode::CONFLICT);
        assert_eq!(
            paused.message,
            "feed is paused; enable it before fetching now"
        );

        let missing = queue_ingest_feed(State(pool), Path(i64::MAX))
            .await
            .unwrap_err();
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        assert_eq!(
            missing.message,
            format!("no feed found with id={}", i64::MAX)
        );
    }

    #[sqlx::test]
    async fn creating_an_existing_paused_feed_returns_it_without_unpausing_or_queueing(
        pool: PgPool,
    ) {
        let url = "https://example.com/paused-create.xml";
        let feed = feed_subscription::upsert_feed_by_url(&pool, url)
            .await
            .unwrap();
        feed_subscription::set_feed_enabled(&pool, feed.id, false)
            .await
            .unwrap();

        let Json(created) = create_feed(State(pool.clone()), Json(NewFeed { url: url.into() }))
            .await
            .unwrap();

        assert_eq!(created.id, feed.id);
        assert!(!created.is_enabled);
        let queued = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM feed_update_queue WHERE feed_id = $1",
            feed.id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(queued, Some(0));
    }

    #[sqlx::test]
    async fn enabling_a_feed_marks_it_enabled_and_queues_it(pool: PgPool) {
        let feed = feed_subscription::upsert_feed_by_url(&pool, "https://example.com/e.xml")
            .await
            .unwrap();
        feed_subscription::set_feed_enabled(&pool, feed.id, false)
            .await
            .unwrap();

        update_feed_enabled(
            State(pool.clone()),
            Path(feed.id),
            Json(UpdateFeedEnabled { is_enabled: true }),
        )
        .await
        .unwrap();

        assert!(
            feed_subscription::read_feed(&pool, feed.id)
                .await
                .unwrap()
                .is_enabled
        );
        let queued = sqlx::query_scalar!(
            "SELECT COUNT(*) FROM feed_update_queue WHERE feed_id = $1",
            feed.id
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(queued, Some(1));
    }
}
