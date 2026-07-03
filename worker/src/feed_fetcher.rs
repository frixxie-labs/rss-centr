use anyhow::{Context, Result};
use feed_rs::model::Feed;
use feed_rs::parser;
use reqwest::{StatusCode, header};
use rss_centr_core::feed_update_queue::DequeuedFeedUpdate;
use tokio::time::Instant;

use crate::telemetry::record_feed_fetch_duration;

pub(crate) enum FetchOutcome {
    NotModified {
        etag: Option<String>,
        last_modified: Option<String>,
    },
    Fetched {
        feed: Box<Feed>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

pub(crate) async fn fetch_feed(
    http: &reqwest::Client,
    feed: &DequeuedFeedUpdate,
) -> Result<FetchOutcome> {
    let started_at = Instant::now();
    let outcome = fetch_feed_inner(http, feed).await;
    let elapsed = started_at.elapsed();

    match &outcome {
        Ok(FetchOutcome::NotModified { .. }) => {
            record_feed_fetch_duration("not_modified", elapsed);
        }
        Ok(FetchOutcome::Fetched { .. }) => {
            record_feed_fetch_duration("fetched", elapsed);
        }
        Err(_) => {
            record_feed_fetch_duration("error", elapsed);
        }
    }

    outcome
}

async fn fetch_feed_inner(
    http: &reqwest::Client,
    feed: &DequeuedFeedUpdate,
) -> Result<FetchOutcome> {
    let mut request = http.get(feed.url.as_str());
    if let Some(etag) = feed.etag.as_deref() {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = feed.last_modified.as_deref() {
        request = request.header(header::IF_MODIFIED_SINCE, last_modified);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("failed to fetch feed from {}", feed.url))?;

    let etag = header_value_to_string(response.headers().get(header::ETAG));
    let last_modified = header_value_to_string(response.headers().get(header::LAST_MODIFIED));

    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(FetchOutcome::NotModified {
            etag,
            last_modified,
        });
    }

    let response = response
        .error_for_status()
        .with_context(|| format!("non-success status fetching feed from {}", feed.url))?;
    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("failed to read response body from {}", feed.url))?;
    let feed = parser::parse(&bytes[..])
        .with_context(|| format!("failed to parse feed from {}", feed.url))?;

    Ok(FetchOutcome::Fetched {
        feed: Box::new(feed),
        etag,
        last_modified,
    })
}

fn header_value_to_string(value: Option<&header::HeaderValue>) -> Option<String> {
    value.and_then(|v| v.to_str().ok()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_header_value_to_string_returns_ascii_header_text() {
        let value = header::HeaderValue::from_static("\"etag-123\"");

        assert_eq!(
            header_value_to_string(Some(&value)),
            Some("\"etag-123\"".to_string())
        );
    }

    #[test]
    fn test_header_value_to_string_none_for_missing_header() {
        assert_eq!(header_value_to_string(None), None);
    }
}
