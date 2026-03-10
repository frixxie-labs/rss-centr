use tokio::sync::mpsc::Sender;

use axum::{Json, extract::State};
use sqlx::SqlitePool;
use tracing::instrument;

use crate::{
    background_tasks::IngestJob,
    feed::{
        feed_item::read_all_feed_items,
        feed_title_index::{FeedTitleIndex, FeedTitleIndexEntry},
    },
    handlers::error::HandlerError,
};

type FeedState = State<(SqlitePool, Sender<IngestJob>)>;

#[instrument]
pub async fn fetch_feed_title_index(
    State(app_state): FeedState,
) -> Result<Json<Vec<FeedTitleIndexEntry>>, HandlerError> {
    let (pool, _tx) = app_state;
    let items = read_all_feed_items(&pool).await.unwrap();
    let index = FeedTitleIndex::from(items);
    Ok(Json(index.export_index()))
}
