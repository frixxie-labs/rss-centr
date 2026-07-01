use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use clap::Parser;
use feed_rs::model::{Entry, Feed, Text};
use feed_rs::parser;
use metrics::{counter, gauge, histogram};
use metrics_exporter_prometheus::PrometheusBuilder;
use reqwest::{StatusCode, header};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult, FeedUpdateItemInput,
};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep};
use tracing::{Level, error, info, warn};
use tracing_subscriber::FmtSubscriber;

#[derive(Debug, Clone)]
enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trace" => Ok(LogLevel::Trace),
            "debug" => Ok(LogLevel::Debug),
            "info" => Ok(LogLevel::Info),
            "warn" => Ok(LogLevel::Warn),
            "error" => Ok(LogLevel::Error),
            _ => Err("unknown log level".to_string()),
        }
    }
}

impl From<LogLevel> for Level {
    fn from(log_level: LogLevel) -> Self {
        match log_level {
            LogLevel::Trace => Level::TRACE,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Info => Level::INFO,
            LogLevel::Warn => Level::WARN,
            LogLevel::Error => Level::ERROR,
        }
    }
}

#[derive(Debug, Parser)]
struct Opts {
    #[arg(long, env = "BACKEND_URL", default_value = "http://localhost:8080")]
    backend_url: String,

    #[arg(long, default_value = "0.0.0.0:9090")]
    metrics_host: String,

    #[arg(long, default_value = "25")]
    limit: i64,

    #[arg(long, default_value = "300")]
    lease_seconds: i64,

    #[arg(long, default_value = "30")]
    idle_sleep_seconds: u64,

    #[arg(long)]
    once: bool,

    #[arg(short, long, default_value = "info")]
    log_level: LogLevel,
}

#[derive(Clone)]
struct QueueClient {
    base_url: String,
    http: reqwest::Client,
}

enum FetchOutcome {
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

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let level: Level = opts.log_level.clone().into();
    let subscriber = FmtSubscriber::builder().with_max_level(level).finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("failed to install tracing subscriber")?;

    if opts.limit <= 0 {
        anyhow::bail!("limit must be positive");
    }
    if opts.lease_seconds <= 0 {
        anyhow::bail!("lease_seconds must be positive");
    }

    let metrics_addr: SocketAddr = opts
        .metrics_host
        .parse()
        .with_context(|| format!("invalid metrics listen address: {}", opts.metrics_host))?;
    PrometheusBuilder::new()
        .with_http_listener(metrics_addr)
        .install()
        .context("failed to install metrics recorder/exporter")?;
    info!("Serving Prometheus metrics on {metrics_addr}");

    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;
    let queue = QueueClient::new(opts.backend_url, http.clone());
    let idle_sleep = Duration::from_secs(opts.idle_sleep_seconds);

    loop {
        let processed = match run_once(&queue, &http, opts.limit, opts.lease_seconds).await {
            Ok(processed) => processed,
            Err(e) if !opts.once => {
                warn!(error = %e, "worker iteration failed; retrying after idle sleep");
                sleep(idle_sleep).await;
                continue;
            }
            Err(e) => return Err(e),
        };
        if opts.once {
            break;
        }
        if processed == 0 {
            sleep(idle_sleep).await;
        }
    }

    Ok(())
}

async fn run_once(
    queue: &QueueClient,
    http: &reqwest::Client,
    limit: i64,
    lease_seconds: i64,
) -> Result<usize> {
    let feeds = queue.dequeue(limit, lease_seconds).await?;
    // Set (not just recorded on activity) so the gauge drops back to 0 once
    // the queue drains, rather than sticking at the last nonzero batch size.
    gauge!("rss_centr_worker_dequeued_feeds").set(feeds.len() as f64);
    if feeds.is_empty() {
        return Ok(0);
    }

    let count = feeds.len();
    let mut tasks = JoinSet::new();
    for feed in feeds {
        let queue = queue.clone();
        let http = http.clone();
        let feed_id = feed.feed_id;
        tasks.spawn(async move { (feed_id, process_feed(&queue, &http, feed).await) });
    }

    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((feed_id, Ok(()))) => {
                info!(feed_id = feed_id, "feed update task finished");
            }
            Ok((feed_id, Err(e))) => {
                warn!(feed_id = feed_id, error = %e, "feed update failed");
            }
            Err(e) => {
                warn!(error = %e, "feed update task panicked");
            }
        }
    }

    Ok(count)
}

async fn process_feed(
    queue: &QueueClient,
    http: &reqwest::Client,
    feed: DequeuedFeedUpdate,
) -> Result<()> {
    let feed_id = feed.feed_id;
    let lease_token = feed.lease_token.clone();

    match fetch_feed(http, &feed).await {
        Ok(FetchOutcome::NotModified {
            etag,
            last_modified,
        }) => {
            let result = queue
                .complete(
                    feed_id,
                    CompleteFeedUpdateRequest {
                        lease_token,
                        fetched: false,
                        title: None,
                        site_url: None,
                        etag,
                        last_modified,
                        items: Vec::new(),
                    },
                )
                .await?;
            record_feed_processed("not_modified");
            info!(
                feed_id = feed_id,
                inserted_items = result.inserted_items,
                next_due_at = %result.next_due_at,
                "feed not modified"
            );
        }
        Ok(FetchOutcome::Fetched {
            feed,
            etag,
            last_modified,
        }) => {
            let items = feed.entries.iter().map(entry_to_item).collect::<Vec<_>>();
            let item_count = items.len();
            let (title, site_url) = feed_title_and_site_url(&feed);
            let result = queue
                .complete(
                    feed_id,
                    CompleteFeedUpdateRequest {
                        lease_token,
                        fetched: true,
                        title,
                        site_url,
                        etag,
                        last_modified,
                        items,
                    },
                )
                .await?;
            record_feed_processed("fetched");
            counter!("rss_centr_worker_feed_items_inserted_total")
                .increment(result.inserted_items as u64);
            info!(
                feed_id = feed_id,
                fetched_items = item_count,
                inserted_items = result.inserted_items,
                next_due_at = %result.next_due_at,
                "feed update completed"
            );
        }
        Err(fetch_error) => {
            record_feed_processed("failed");
            let failed = queue
                .failed(feed_id, FailedFeedUpdateRequest { lease_token })
                .await;
            match failed {
                Ok(result) => {
                    warn!(
                        feed_id = feed_id,
                        next_due_at = %result.next_due_at,
                        error = %fetch_error,
                        "feed update rescheduled after fetch failure"
                    );
                }
                Err(fail_error) => {
                    error!(
                        feed_id = feed_id,
                        fetch_error = %fetch_error,
                        fail_error = %fail_error,
                        "failed to record feed update failure"
                    );
                    return Err(fail_error);
                }
            }
        }
    }

    Ok(())
}

async fn fetch_feed(http: &reqwest::Client, feed: &DequeuedFeedUpdate) -> Result<FetchOutcome> {
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

/// Duration of a single `fetch_feed` call (network request + parse),
/// labeled by outcome. Mirrors the old backend-side
/// `rss_centr_feed_source_fetch_duration_seconds` metric, which measured the
/// same thing before fetching moved from the backend's ingest pipeline into
/// this worker.
fn record_feed_fetch_duration(outcome: &str, elapsed: Duration) {
    let labels = [("outcome", outcome.to_string())];
    histogram!("rss_centr_worker_feed_fetch_duration_seconds", &labels).record(elapsed);
}

/// Count of feeds this worker finished processing, labeled by outcome:
/// "fetched" (new content), "not_modified" (304/cache-validated unchanged),
/// or "failed" (fetch error, rescheduled with backoff).
fn record_feed_processed(outcome: &str) {
    let labels = [("outcome", outcome.to_string())];
    counter!("rss_centr_worker_feeds_processed_total", &labels).increment(1);
}

impl QueueClient {
    fn new(base_url: String, http: reqwest::Client) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http,
        }
    }

    async fn dequeue(&self, limit: i64, lease_seconds: i64) -> Result<Vec<DequeuedFeedUpdate>> {
        let url = format!(
            "{}/internal/feed-update-queue/dequeue?limit={limit}&lease_seconds={lease_seconds}",
            self.base_url
        );
        let response = self
            .http
            .post(url)
            .send()
            .await
            .context("failed to call dequeue endpoint")?;
        if !response.status().is_success() {
            return Err(response_error("dequeue feed updates", response).await);
        }

        response
            .json::<Vec<DequeuedFeedUpdate>>()
            .await
            .context("failed to decode dequeue response")
    }

    async fn complete(
        &self,
        feed_id: i64,
        request: CompleteFeedUpdateRequest,
    ) -> Result<CompleteFeedUpdateResult> {
        let url = format!(
            "{}/internal/feed-update-queue/{feed_id}/complete",
            self.base_url
        );
        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("failed to call complete endpoint for feed_id={feed_id}"))?;
        if !response.status().is_success() {
            return Err(response_error("complete feed update", response).await);
        }

        response
            .json::<CompleteFeedUpdateResult>()
            .await
            .with_context(|| format!("failed to decode complete response for feed_id={feed_id}"))
    }

    async fn failed(
        &self,
        feed_id: i64,
        request: FailedFeedUpdateRequest,
    ) -> Result<FailedFeedUpdateResult> {
        let url = format!(
            "{}/internal/feed-update-queue/{feed_id}/failed",
            self.base_url
        );
        let response = self
            .http
            .post(url)
            .json(&request)
            .send()
            .await
            .with_context(|| format!("failed to call failed endpoint for feed_id={feed_id}"))?;
        if !response.status().is_success() {
            return Err(response_error("record feed update failure", response).await);
        }

        response
            .json::<FailedFeedUpdateResult>()
            .await
            .with_context(|| format!("failed to decode failed response for feed_id={feed_id}"))
    }
}

async fn response_error(action: &str, response: reqwest::Response) -> anyhow::Error {
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    anyhow!("{action} failed with status {status}: {body}")
}

fn entry_to_item(entry: &Entry) -> FeedUpdateItemInput {
    let now = Utc::now();
    let (summary, content) = entry_summary_and_content(entry);

    FeedUpdateItemInput {
        external_id: entry_external_id(entry),
        title: entry
            .title
            .as_ref()
            .map(text_value)
            .unwrap_or("(no title)")
            .to_string(),
        url: entry
            .links
            .first()
            .map(|link| link.href.clone())
            .unwrap_or_default(),
        summary: Some(summary),
        content: Some(content),
        author: Some(
            entry
                .authors
                .first()
                .map(|author| author.name.clone())
                .unwrap_or_default(),
        ),
        published_at: Some(entry_published_at(entry).unwrap_or(now)),
    }
}

fn text_value(t: &Text) -> &str {
    t.content.as_str()
}

fn feed_title_and_site_url(feed: &Feed) -> (Option<String>, Option<String>) {
    let title = feed.title.as_ref().map(|t| text_value(t).to_string());
    let site_url = feed.links.first().map(|link| link.href.clone());
    (title, site_url)
}

fn entry_external_id(entry: &Entry) -> String {
    if !entry.id.is_empty() {
        return entry.id.clone();
    }

    if let Some(link) = entry.links.first() {
        return link.href.clone();
    }

    let title = entry
        .title
        .as_ref()
        .map(text_value)
        .unwrap_or("")
        .to_string();
    let published = entry_published_at(entry)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();

    format!("fallback:{title}:{published}")
}

fn entry_summary_and_content(entry: &Entry) -> (String, String) {
    let summary = entry
        .summary
        .as_ref()
        .map(text_value)
        .unwrap_or("")
        .to_string();

    let content = entry
        .content
        .as_ref()
        .and_then(|content| content.body.as_deref())
        .unwrap_or("")
        .to_string();

    (summary, content)
}

fn entry_published_at(entry: &Entry) -> Option<DateTime<Utc>> {
    entry
        .published
        .or(entry.updated)
        .map(|dt| dt.with_timezone(&Utc))
}

fn header_value_to_string(value: Option<&header::HeaderValue>) -> Option<String> {
    value.and_then(|v| v.to_str().ok()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use feed_rs::model::Link;
    use quickcheck::TestResult;

    fn link(href: String) -> Link {
        Link {
            href,
            rel: None,
            media_type: None,
            href_lang: None,
            title: None,
            length: None,
        }
    }

    fn datetime(seconds: i32) -> DateTime<Utc> {
        DateTime::from_timestamp(i64::from(seconds), 0).unwrap()
    }

    // These mirror the equivalent properties in `backend/src/feed/ingest.rs`
    // for the same fallback logic, duplicated here because the worker has no
    // compile-time dependency on the backend crate.
    quickcheck::quickcheck! {
        fn prop_entry_external_id_prefers_non_empty_id(id: String, href: String) -> TestResult {
            if id.is_empty() {
                return TestResult::discard();
            }

            let entry = Entry {
                id: id.clone(),
                links: vec![link(href)],
                ..Default::default()
            };

            TestResult::from_bool(entry_external_id(&entry) == id)
        }

        fn prop_entry_external_id_uses_first_link_when_id_is_empty(href: String, other_href: String) -> bool {
            let entry = Entry {
                id: String::new(),
                links: vec![link(href.clone()), link(other_href)],
                ..Default::default()
            };

            entry_external_id(&entry) == href
        }

        fn prop_entry_published_at_prefers_published(published_seconds: i32, updated_seconds: i32) -> bool {
            let published = datetime(published_seconds);
            let updated = datetime(updated_seconds);
            let entry = Entry {
                published: Some(published),
                updated: Some(updated),
                ..Default::default()
            };

            entry_published_at(&entry) == Some(published)
        }

        fn prop_entry_published_at_uses_updated_without_published(updated_seconds: i32) -> bool {
            let updated = datetime(updated_seconds);
            let entry = Entry {
                updated: Some(updated),
                ..Default::default()
            };

            entry_published_at(&entry) == Some(updated)
        }
    }

    // -----------------------------------------------------------------------
    // `text_value`, `entry_summary_and_content`, `feed_title_and_site_url` and
    // `entry_to_item` all read data out of `feed_rs::model::{Feed, Text}`
    // values, but those types have no public constructor or `Default` impl
    // outside the `feed_rs` crate itself (by design, since only its own
    // parser is meant to build them). So rather than fighting that, these
    // tests drive the real parser with a small fixture document and assert
    // on what we hand off to the backend -- which also happens to exercise
    // the exact code path (`parser::parse` output feeding `entry_to_item`)
    // that `fetch_feed` uses in production.
    // -----------------------------------------------------------------------

    const SAMPLE_ATOM_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Feed</title>
  <link href="https://example.com" rel="alternate"/>
  <id>urn:uuid:example-feed</id>
  <entry>
    <title>First Post</title>
    <link href="https://example.com/1" rel="alternate"/>
    <id>https://example.com/1</id>
    <summary>Summary of first post</summary>
    <content type="html">Full content</content>
    <author><name>Jane Doe</name></author>
    <published>2026-07-01T10:00:00Z</published>
  </entry>
  <entry>
    <link href="https://example.com/2" rel="alternate"/>
    <id>https://example.com/2</id>
  </entry>
</feed>"#;

    fn parse_sample() -> Feed {
        parser::parse(SAMPLE_ATOM_FEED.as_bytes()).expect("sample feed should parse")
    }

    #[test]
    fn test_feed_title_and_site_url_reads_feed_metadata() {
        let feed = parse_sample();
        let (title, site_url) = feed_title_and_site_url(&feed);

        assert_eq!(title.as_deref(), Some("Example Feed"));
        // feed-rs normalizes a bare-domain href by appending a trailing
        // slash (via the `url` crate), even though the fixture below
        // writes `href="https://example.com"` with none.
        assert_eq!(site_url.as_deref(), Some("https://example.com/"));
    }

    #[test]
    fn test_text_value_returns_underlying_content() {
        let feed = parse_sample();
        let title = feed.title.as_ref().expect("feed has a title");

        assert_eq!(text_value(title), "Example Feed");
    }

    #[test]
    fn test_entry_to_item_maps_fully_populated_entry() {
        let feed = parse_sample();
        let entry = &feed.entries[0];

        let item = entry_to_item(entry);

        assert_eq!(item.external_id, "https://example.com/1");
        assert_eq!(item.title, "First Post");
        assert_eq!(item.url, "https://example.com/1");
        assert_eq!(item.summary.as_deref(), Some("Summary of first post"));
        assert_eq!(item.content.as_deref(), Some("Full content"));
        assert_eq!(item.author.as_deref(), Some("Jane Doe"));
        assert_eq!(
            item.published_at,
            Some(
                DateTime::parse_from_rfc3339("2026-07-01T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[test]
    fn test_entry_to_item_falls_back_for_sparse_entry() {
        let feed = parse_sample();
        let entry = &feed.entries[1];

        let item = entry_to_item(entry);

        assert_eq!(item.external_id, "https://example.com/2");
        // No <title>: falls back to a placeholder instead of panicking.
        assert_eq!(item.title, "(no title)");
        // No <summary>/<content>/<author>: the worker always sends `Some("")`
        // rather than `None` for missing detail fields. This matters because
        // the backend's `item_has_detail` check only skips inserting a detail
        // row when every one of these is `None` -- which this producer never
        // sends, so that skip path is currently unreachable in production.
        assert_eq!(item.summary.as_deref(), Some(""));
        assert_eq!(item.content.as_deref(), Some(""));
        assert_eq!(item.author.as_deref(), Some(""));
        // No <published>: falls back to "now" rather than `None`.
        assert!(item.published_at.is_some());
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
