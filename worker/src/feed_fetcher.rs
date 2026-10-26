use std::future::Future;

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use feed_rs::model::Feed;
use reqwest::{StatusCode, header};
use rss_centr_core::feed_update_queue::DequeuedFeedUpdate;
use tokio::time::Instant;

use crate::telemetry::record_feed_fetch_duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchFailureReason {
    /// The publisher answered with a bot challenge, whatever the status code.
    BotProtection,
    /// The publisher answered with a non-success status.
    HttpStatus,
}

#[derive(Debug)]
pub(crate) struct FeedFetchFailure {
    pub(crate) status: StatusCode,
    pub(crate) reason: FetchFailureReason,
    pub(crate) retry_after: Option<DateTime<Utc>>,
}

impl std::fmt::Display for FeedFetchFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            FetchFailureReason::BotProtection => write!(
                f,
                "Blocked by publisher's bot protection (HTTP {})",
                self.status
            ),
            FetchFailureReason::HttpStatus => {
                write!(f, "non-success status fetching feed (HTTP {})", self.status)
            }
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

/// Largest feed body the worker will download.
pub(crate) const MAX_FEED_BYTES: usize = 20 * 1024 * 1024;

pub(crate) const USER_AGENT: &str = concat!(
    "rss-centr-worker/",
    env!("CARGO_PKG_VERSION"),
    " (RSS aggregator)"
);

/// Client settings shared by every outgoing request of the worker.
pub(crate) fn http_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(5))
}

/// Bot-protection headers that mark a challenge page instead of the feed.
const BOT_CHALLENGE_HEADERS: [&str; 2] = ["x-vercel-mitigated", "cf-mitigated"];

/// Fetches and parses feeds, abstracted so the runner can be tested.
pub(crate) trait FeedSource: Send + Sync + 'static {
    fn fetch(&self, feed: &DequeuedFeedUpdate)
    -> impl Future<Output = Result<FetchOutcome>> + Send;
}

pub(crate) struct HttpFeedSource {
    http: reqwest::Client,
    max_bytes: usize,
}

impl HttpFeedSource {
    pub(crate) fn new(http: reqwest::Client) -> Self {
        Self {
            http,
            max_bytes: MAX_FEED_BYTES,
        }
    }
}

impl FeedSource for HttpFeedSource {
    async fn fetch(&self, feed: &DequeuedFeedUpdate) -> Result<FetchOutcome> {
        fetch_feed(&self.http, feed, self.max_bytes).await
    }
}

pub(crate) async fn fetch_feed(
    http: &reqwest::Client,
    feed: &DequeuedFeedUpdate,
    max_bytes: usize,
) -> Result<FetchOutcome> {
    let started_at = Instant::now();
    let outcome = fetch_feed_inner(http, feed, max_bytes).await;
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
    max_bytes: usize,
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

    match classify_response(response.status(), response.headers(), Utc::now()) {
        ResponseClass::NotModified => {
            return Ok(FetchOutcome::NotModified {
                etag,
                last_modified,
            });
        }
        ResponseClass::Failed(failure) => {
            if let Err(error) = response.error_for_status_ref() {
                return Err(anyhow::Error::new(error).context(failure));
            }
            return Err(failure.into());
        }
        ResponseClass::Feed => {}
    }

    let bytes = read_body_limited(response, max_bytes)
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

/// What a response's status and headers say before its body is read.
#[derive(Debug)]
enum ResponseClass {
    NotModified,
    Failed(FeedFetchFailure),
    /// The body should hold the feed.
    Feed,
}

fn classify_response(
    status: StatusCode,
    headers: &header::HeaderMap,
    now: DateTime<Utc>,
) -> ResponseClass {
    let failure = |reason| {
        ResponseClass::Failed(FeedFetchFailure {
            status,
            reason,
            retry_after: parse_retry_after(headers.get(header::RETRY_AFTER), now),
        })
    };
    if status == StatusCode::NOT_MODIFIED {
        ResponseClass::NotModified
    } else if is_bot_challenge(headers) {
        failure(FetchFailureReason::BotProtection)
    } else if !status.is_success() {
        failure(FetchFailureReason::HttpStatus)
    } else {
        ResponseClass::Feed
    }
}

fn is_bot_challenge(headers: &header::HeaderMap) -> bool {
    BOT_CHALLENGE_HEADERS.iter().any(|name| {
        headers
            .get(*name)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("challenge"))
    })
}

async fn read_body_limited(mut response: reqwest::Response, max_bytes: usize) -> Result<Vec<u8>> {
    if let Some(length) = response.content_length()
        && length > max_bytes as u64
    {
        bail!("feed body of {length} bytes exceeds the {max_bytes} byte limit");
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if body.len() + chunk.len() > max_bytes {
            bail!("feed body exceeds the {max_bytes} byte limit");
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
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
        let http = http_client_builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap();
        let outcome = fetch_feed(&http, &feed, MAX_FEED_BYTES)
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

    struct LocalResponse<'a> {
        status: &'a str,
        headers: &'a str,
        body: &'a str,
        content_length: bool,
        max_bytes: usize,
    }

    impl<'a> LocalResponse<'a> {
        fn new(status: &'a str, headers: &'a str, body: &'a str) -> Self {
            Self {
                status,
                headers,
                body,
                content_length: true,
                max_bytes: MAX_FEED_BYTES,
            }
        }
    }

    async fn fetch_local_response(status: &str, headers: &str, body: &str) -> Result<FetchOutcome> {
        fetch_local(LocalResponse::new(status, headers, body))
            .await
            .0
    }

    /// Serves one canned response; returns the outcome and the raw request.
    async fn fetch_local(local: LocalResponse<'_>) -> (Result<FetchOutcome>, String) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let length = if local.content_length {
            format!("Content-Length: {}\r\n", local.body.len())
        } else {
            String::new()
        };
        let response = format!(
            "HTTP/1.1 {}\r\n{}{length}Connection: close\r\n\r\n{}",
            local.status, local.headers, local.body
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
            // The client may hang up early (e.g. body too large).
            let _ = socket.write_all(response.as_bytes()).await;
            String::from_utf8(request).unwrap()
        });
        let http = http_client_builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let feed = feed_update(&format!("http://{address}/for-pressen/rss"));

        let outcome = fetch_feed(&http, &feed, local.max_bytes).await;
        (outcome, server.await.unwrap())
    }

    const MINIMAL_RSS: &str = r#"<rss version="2.0"><channel><title>t</title><item><title>a</title><guid>1</guid></item></channel></rss>"#;

    #[tokio::test]
    async fn test_fetch_sends_user_agent() {
        let (outcome, request) = fetch_local(LocalResponse::new("200 OK", "", MINIMAL_RSS)).await;
        assert!(outcome.is_ok());
        assert!(
            request
                .to_ascii_lowercase()
                .contains(&format!("user-agent: {}", USER_AGENT.to_ascii_lowercase())),
            "{request}"
        );
    }

    #[tokio::test]
    async fn test_fetch_rejects_body_over_limit() {
        for content_length in [true, false] {
            let (outcome, _) = fetch_local(LocalResponse {
                content_length,
                max_bytes: 16,
                ..LocalResponse::new("200 OK", "", MINIMAL_RSS)
            })
            .await;
            let error = match outcome {
                Err(error) => format!("{error:#}"),
                Ok(_) => panic!("expected oversized body to fail"),
            };
            assert!(error.contains("16 byte limit"), "{error}");
        }
    }

    #[tokio::test]
    async fn test_fetch_detects_cloudflare_challenge() {
        let error = match fetch_local_response(
            "403 Forbidden",
            "cf-mitigated: challenge\r\n",
            "<html>Just a moment...</html>",
        )
        .await
        {
            Err(error) => error,
            Ok(_) => panic!("expected the challenge to fail"),
        };
        let failure = error.downcast_ref::<FeedFetchFailure>().unwrap();
        assert_eq!(failure.reason, FetchFailureReason::BotProtection);
        assert_eq!(failure.status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_fetch_rejects_unfollowed_redirect_status() {
        let error = match fetch_local_response("300 Multiple Choices", "", MINIMAL_RSS).await {
            Err(error) => error,
            Ok(_) => panic!("expected a 300 response to fail"),
        };
        let failure = error.downcast_ref::<FeedFetchFailure>().unwrap();
        assert_eq!(failure.status, StatusCode::MULTIPLE_CHOICES);
        assert_eq!(failure.reason, FetchFailureReason::HttpStatus);
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
            assert_eq!(failure.reason, FetchFailureReason::BotProtection);
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
            assert_eq!(failure.reason, FetchFailureReason::HttpStatus);
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
    fn classify_response_checks_not_modified_then_bot_protection_then_status() {
        let now = DateTime::UNIX_EPOCH;
        let challenge = header::HeaderMap::from_iter([(
            header::HeaderName::from_static("cf-mitigated"),
            header::HeaderValue::from_static("challenge"),
        )]);
        let plain = header::HeaderMap::new();
        for (status, headers, expected) in [
            (StatusCode::NOT_MODIFIED, &challenge, "not modified"),
            (StatusCode::OK, &challenge, "bot protection"),
            (StatusCode::FORBIDDEN, &challenge, "bot protection"),
            (StatusCode::FORBIDDEN, &plain, "http status"),
            (StatusCode::MOVED_PERMANENTLY, &plain, "http status"),
            (StatusCode::OK, &plain, "feed"),
        ] {
            let actual = match classify_response(status, headers, now) {
                ResponseClass::NotModified => "not modified",
                ResponseClass::Failed(FeedFetchFailure {
                    reason: FetchFailureReason::BotProtection,
                    status: failed,
                    ..
                }) if failed == status => "bot protection",
                ResponseClass::Failed(FeedFetchFailure {
                    reason: FetchFailureReason::HttpStatus,
                    status: failed,
                    ..
                }) if failed == status => "http status",
                ResponseClass::Failed(_) => "wrong status",
                ResponseClass::Feed => "feed",
            };
            assert_eq!(actual, expected, "{status}");
        }
    }

    quickcheck::quickcheck! {
        fn prop_numeric_retry_after_is_seconds_from_now(seconds: u32, padding: u8) -> bool {
            let now = DateTime::UNIX_EPOCH;
            let pad = " ".repeat(usize::from(padding % 3));
            let value = header::HeaderValue::from_str(&format!("{pad}{seconds}{pad}")).unwrap();
            parse_retry_after(Some(&value), now)
                == Some(now + chrono::Duration::seconds(i64::from(seconds)))
        }

        fn prop_retry_after_without_digits_or_date_is_ignored(text: String) -> bool {
            let text: String = text.chars().filter(char::is_ascii_alphabetic).collect();
            let Ok(value) = header::HeaderValue::from_str(&text) else {
                return true;
            };
            parse_retry_after(Some(&value), DateTime::UNIX_EPOCH).is_none()
        }
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
