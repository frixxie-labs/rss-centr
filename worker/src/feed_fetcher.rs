use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use feed_rs::model::Feed;
use reqwest::{StatusCode, header};
use rss_centr_core::feed_update_queue::DequeuedFeedUpdate;
use tokio::time::Instant;

use crate::telemetry::record_feed_fetch_duration;

#[derive(Debug)]
pub(crate) struct FeedFetchFailure {
    pub(crate) status: StatusCode,
    pub(crate) blocked_by_bot_protection: bool,
    pub(crate) retry_after: Option<DateTime<Utc>>,
}

impl std::fmt::Display for FeedFetchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.blocked_by_bot_protection {
            write!(
                f,
                "Blocked by publisher's bot protection (HTTP {})",
                self.status
            )
        } else {
            write!(f, "non-success status fetching feed (HTTP {})", self.status)
        }
    }
}

impl std::error::Error for FeedFetchFailure {}

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

    let blocked_by_bot_protection = response
        .headers()
        .get("x-vercel-mitigated")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case("challenge"));
    if blocked_by_bot_protection
        || response.status().is_client_error()
        || response.status().is_server_error()
    {
        let failure = FeedFetchFailure {
            status: response.status(),
            blocked_by_bot_protection,
            retry_after: parse_retry_after(response.headers().get(header::RETRY_AFTER), Utc::now()),
        };
        if let Err(error) = response.error_for_status_ref() {
            return Err(anyhow::Error::new(error).context(failure));
        }
        return Err(failure.into());
    }

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
    let feed = crate::feed_parser::parse_feed(&bytes)
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

fn parse_retry_after(
    value: Option<&header::HeaderValue>,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let value = value?.to_str().ok()?.trim();
    if !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()) {
        let seconds = value.parse::<i64>().ok()?;
        return now.checked_add_signed(chrono::Duration::try_seconds(seconds)?);
    }
    DateTime::parse_from_rfc2822(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_update(url: &str) -> DequeuedFeedUpdate {
        DequeuedFeedUpdate {
            feed_id: 1,
            url: url.to_string(),
            title: None,
            site_url: None,
            etag: None,
            last_modified: None,
            poll_interval_seconds: 300,
            last_checked_at: None,
            last_success_at: None,
            last_inserted_at: None,
            failure_count: 0,
            lease_token: String::new(),
            lease_expires_at: chrono::Utc::now(),
        }
    }

    async fn assert_live_feed(url: &str) {
        let feed = feed_update(url);
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap();
        let outcome = fetch_feed(&http, &feed)
            .await
            .unwrap_or_else(|error| panic!("{error:#}"));
        match outcome {
            FetchOutcome::Fetched { feed, .. } => assert!(!feed.entries.is_empty()),
            FetchOutcome::NotModified { .. } => panic!("expected a fresh feed"),
        }
    }

    #[tokio::test]
    #[ignore = "requires access to NRK's live feed"]
    async fn test_fetch_live_nrk_feed() {
        assert_live_feed("https://www.nrk.no/nyheter/siste.rss").await;
    }

    #[tokio::test]
    #[ignore = "requires access to Kongehuset's live feed; may be blocked by Vercel"]
    async fn test_fetch_live_kongehuset_feed() {
        assert_live_feed("https://www.kongehuset.no/for-pressen/rss").await;
    }

    async fn fetch_local_response(status: &str, headers: &str, body: &str) -> Result<FetchOutcome> {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = format!(
            "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.unwrap();
                assert!(read > 0, "connection closed before request headers");
                request.extend_from_slice(&buffer[..read]);
            }
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let http = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let feed = feed_update(&format!("http://{address}/for-pressen/rss"));

        let outcome = fetch_feed(&http, &feed).await;
        server.await.unwrap();
        outcome
    }

    #[tokio::test]
    async fn test_fetch_rejects_security_checkpoint_before_parsing() {
        for status in ["429 Too Many Requests", "200 OK"] {
            let error = match fetch_local_response(
                status,
                "Content-Type: text/html; charset=utf-8\r\nx-vercel-mitigated: challenge\r\nRetry-After: 120\r\n",
                "<html><title>Vercel Security Checkpoint</title></html>",
            ).await {
                Err(error) => error,
                Ok(_) => panic!("expected the security checkpoint to fail"),
            };
            assert!(
                error
                    .to_string()
                    .contains("Blocked by publisher's bot protection")
            );
            let failure = error.downcast_ref::<FeedFetchFailure>().unwrap();
            assert!(failure.blocked_by_bot_protection);
            assert!(failure.retry_after.is_some());
            if status.starts_with("429") {
                assert_eq!(
                    error.downcast_ref::<reqwest::Error>().unwrap().status(),
                    Some(StatusCode::TOO_MANY_REQUESTS)
                );
            }
        }
    }

    #[tokio::test]
    async fn test_fetch_http_failure_preserves_retry_after_without_claiming_bot_protection() {
        let retry_after = "Wed, 01 Jul 2026 10:00:00 GMT";
        for status in ["429 Too Many Requests", "503 Service Unavailable"] {
            let error = match fetch_local_response(
                status,
                &format!("Retry-After: {retry_after}\r\n"),
                "unavailable",
            )
            .await
            {
                Err(error) => error,
                Ok(_) => panic!("expected HTTP failure"),
            };
            let failure = error.downcast_ref::<FeedFetchFailure>().unwrap();
            assert!(!failure.blocked_by_bot_protection);
            assert_eq!(
                failure.retry_after,
                Some(
                    DateTime::parse_from_rfc2822(retry_after)
                        .unwrap()
                        .with_timezone(&Utc)
                )
            );
            assert!(
                error
                    .to_string()
                    .contains("non-success status fetching feed")
            );
        }
    }

    #[test]
    fn test_parse_retry_after() {
        let now = DateTime::parse_from_rfc3339("2026-07-01T09:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        for (value, expected) in [
            ("120", Some(now + chrono::Duration::seconds(120))),
            ("0", Some(now)),
            (
                "Wed, 01 Jul 2026 10:00:00 GMT",
                Some(now + chrono::Duration::hours(1)),
            ),
            (
                "Wed, 01 Jul 2026 08:00:00 GMT",
                Some(now - chrono::Duration::hours(1)),
            ),
            ("invalid", None),
            ("-1", None),
            ("", None),
            ("9223372036854775807", None),
            ("999999999999999999999999999999", None),
        ] {
            assert_eq!(
                parse_retry_after(Some(&header::HeaderValue::from_str(value).unwrap()), now),
                expected,
                "{value}"
            );
        }
        assert_eq!(parse_retry_after(None, now), None);
    }

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
