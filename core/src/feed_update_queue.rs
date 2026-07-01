use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DequeuedFeedUpdate {
    pub feed_id: i64,
    pub url: String,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub poll_interval_seconds: i64,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_inserted_at: Option<DateTime<Utc>>,
    pub failure_count: i64,
    pub lease_token: String,
    pub lease_expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FeedUpdateItemInput {
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CompleteFeedUpdateRequest {
    pub lease_token: String,
    /// Whether the feed body was actually fetched and parsed (`true`), as
    /// opposed to the origin server responding that the feed is unchanged
    /// (`false`, e.g. HTTP 304 Not Modified). Drives whether metadata/cadence
    /// are refreshed or the poll interval is backed off.
    pub fetched: bool,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub items: Vec<FeedUpdateItemInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FailedFeedUpdateRequest {
    pub lease_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CompleteFeedUpdateResult {
    pub inserted_items: i64,
    pub next_due_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FailedFeedUpdateResult {
    pub next_due_at: DateTime<Utc>,
}
