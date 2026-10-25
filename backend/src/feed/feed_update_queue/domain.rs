use std::{error::Error, fmt};

use rss_centr_core::feed_update_queue::{CompleteFeedUpdateRequest, FeedUpdateItemInput};

#[derive(Debug)]
pub enum FeedQueueError {
    LeaseConflict { feed_id: i64 },
    FeedPaused { feed_id: i64 },
    FeedNotFound { feed_id: i64 },
    Storage(anyhow::Error),
}

impl fmt::Display for FeedQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LeaseConflict { feed_id } => {
                write!(f, "feed update lease conflict for feed_id={feed_id}")
            }
            Self::FeedPaused { feed_id } => write!(f, "feed is paused for feed_id={feed_id}"),
            Self::FeedNotFound { feed_id } => write!(f, "no feed found with id={feed_id}"),
            Self::Storage(error) => error.fmt(f),
        }
    }
}

impl Error for FeedQueueError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for FeedQueueError {
    fn from(error: anyhow::Error) -> Self {
        Self::Storage(error)
    }
}

/// The wire flag selects polling policy, not which payload fields are accepted.
pub(super) enum FeedUpdateCompletion {
    Fetched(CompletionData),
    NotModified(CompletionData),
}

pub(super) struct CompletionData {
    pub(super) lease_token: String,
    pub(super) title: Option<String>,
    pub(super) site_url: Option<String>,
    pub(super) etag: Option<String>,
    pub(super) last_modified: Option<String>,
    pub(super) items: Vec<FeedUpdateItemInput>,
}

impl From<CompleteFeedUpdateRequest> for FeedUpdateCompletion {
    fn from(request: CompleteFeedUpdateRequest) -> Self {
        let CompleteFeedUpdateRequest {
            lease_token,
            fetched,
            title,
            site_url,
            etag,
            last_modified,
            items,
        } = request;
        let data = CompletionData {
            lease_token,
            title,
            site_url,
            etag,
            last_modified,
            items,
        };
        if fetched {
            Self::Fetched(data)
        } else {
            Self::NotModified(data)
        }
    }
}

impl FeedUpdateCompletion {
    pub(super) fn data(&self) -> &CompletionData {
        match self {
            Self::Fetched(data) | Self::NotModified(data) => data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_conversion_selects_the_outcome_without_dropping_wire_fields() {
        for fetched in [true, false] {
            let request = CompleteFeedUpdateRequest {
                lease_token: "lease".into(),
                fetched,
                title: Some("Title".into()),
                site_url: Some("https://example.com".into()),
                etag: Some("etag".into()),
                last_modified: Some("last-modified".into()),
                items: vec![FeedUpdateItemInput {
                    external_id: "one".into(),
                    title: "Item".into(),
                    url: "https://example.com/one".into(),
                    summary: Some("Summary".into()),
                    content: Some("Content".into()),
                    author: Some("Author".into()),
                    published_at: Some(chrono::DateTime::UNIX_EPOCH),
                }],
            };

            let completion = FeedUpdateCompletion::from(request.clone());

            assert_eq!(
                matches!(completion, FeedUpdateCompletion::Fetched(_)),
                fetched
            );
            let data = completion.data();
            assert_eq!(data.lease_token, request.lease_token);
            assert_eq!(data.title, request.title);
            assert_eq!(data.site_url, request.site_url);
            assert_eq!(data.etag, request.etag);
            assert_eq!(data.last_modified, request.last_modified);
            assert_eq!(
                serde_json::to_value(&data.items).unwrap(),
                serde_json::to_value(request.items).unwrap()
            );
        }
    }
}
