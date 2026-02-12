use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::feed::feed_item::FeedItem;

#[derive(Debug, Clone, Serialize)]
pub struct NewFeedItemEvent {
    pub id: i64,
    pub feed_id: i64,
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub inserted_at: DateTime<Utc>,
}

impl From<&FeedItem> for NewFeedItemEvent {
    fn from(item: &FeedItem) -> Self {
        Self {
            id: item.id,
            feed_id: item.feed_id,
            external_id: item.external_id.clone(),
            title: item.title.clone(),
            url: item.url.clone(),
            inserted_at: item.inserted_at,
        }
    }
}
