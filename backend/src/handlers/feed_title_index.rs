use tokio::sync::mpsc::Sender;

use axum::{Json, extract::State};
use sqlx::PgPool;
use tracing::{instrument, warn};

use crate::{
    background_tasks::IngestJob,
    feed::feed_title_index::{FeedTitleIndex, FeedTitleIndexEntry},
    handlers::error::HandlerError,
};

type FeedState = State<(PgPool, Sender<IngestJob>)>;

#[instrument]
pub async fn fetch_feed_title_index(
    State(app_state): FeedState,
) -> Result<Json<Vec<FeedTitleIndexEntry>>, HandlerError> {
    let (pool, _tx) = app_state;
    let items = crate::feed::feed_item::read_all_feed_items(&pool)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to read feed items")
        })?;
    let index = FeedTitleIndex::from(items);
    Ok(Json(index.export_index()))
}

#[utoipa::path(
    get,
    path = "/api/feeds/index/today",
    responses(
        (status = 200, description = "Title index built from feed items in the last 24 hours", body = [FeedTitleIndexEntry]),
        (status = 500, description = "Internal server error"),
    ),
    tag = "feeds"
)]
#[instrument]
pub async fn fetch_recent_feed_title_index(
    State(app_state): FeedState,
) -> Result<Json<Vec<FeedTitleIndexEntry>>, HandlerError> {
    let (pool, _tx) = app_state;
    let index = FeedTitleIndex::build_from_recent(&pool)
        .await
        .map_err(|e| {
            warn!("failed with error: {e:#}");
            HandlerError::from_db(e, "Failed to build recent title index")
        })?;
    Ok(Json(index.export_index()))
}
