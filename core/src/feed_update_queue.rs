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

/// What a completed feed update found. Serialized as a boolean (`true` for
/// [`Self::Fetched`]) so the wire format stays a plain `fetched` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CompletionKind {
    /// The feed body was fetched and parsed.
    Fetched,
    /// The publisher reported the feed unchanged.
    NotModified,
}

impl From<bool> for CompletionKind {
    fn from(fetched: bool) -> Self {
        if fetched {
            Self::Fetched
        } else {
            Self::NotModified
        }
    }
}

impl From<CompletionKind> for bool {
    fn from(kind: CompletionKind) -> Self {
        kind == CompletionKind::Fetched
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CompleteFeedUpdateRequest {
    pub lease_token: String,
    /// Sent as the boolean `fetched`: whether the feed body was actually
    /// fetched and parsed (`true`), as opposed to the origin server responding
    /// that the feed is unchanged (`false`, e.g. HTTP 304 Not Modified). Drives
    /// whether metadata/cadence are refreshed or the poll interval is backed off.
    #[serde(rename = "fetched")]
    #[schema(value_type = bool)]
    pub kind: CompletionKind,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub items: Vec<FeedUpdateItemInput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct FailedFeedUpdateRequest {
    pub lease_token: String,
    /// Do not retry before this time, if supplied by the publisher's Retry-After header.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_after: Option<DateTime<Utc>>,
    /// HTTP status returned by the publisher, if the fetch got a response.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_status: Option<u16>,
    /// Human-readable description of why the fetch failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_kind_keeps_the_boolean_fetched_wire_flag() {
        for (kind, fetched) in [
            (CompletionKind::Fetched, true),
            (CompletionKind::NotModified, false),
        ] {
            let wire = serde_json::json!({
                "lease_token": "lease",
                "fetched": fetched,
                "title": null,
                "site_url": null,
                "etag": null,
                "last_modified": null,
                "items": [],
            });
            let request: CompleteFeedUpdateRequest = serde_json::from_value(wire.clone()).unwrap();
            assert_eq!(request.kind, kind);
            assert_eq!(serde_json::to_value(&request).unwrap(), wire);
        }
    }
}
