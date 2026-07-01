use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Utc};
use clap::Parser;
use feed_rs::model::{Entry, Feed, Text};
use feed_rs::parser;
use reqwest::{StatusCode, header};
use rss_centr_core::feed_update_queue::{
    CompleteFeedUpdateRequest, CompleteFeedUpdateResult, DequeuedFeedUpdate,
    FailedFeedUpdateRequest, FailedFeedUpdateResult, FeedUpdateItemInput,
};
use tokio::task::JoinSet;
use tokio::time::sleep;
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
            info!(
                feed_id = feed_id,
                fetched_items = item_count,
                inserted_items = result.inserted_items,
                next_due_at = %result.next_due_at,
                "feed update completed"
            );
        }
        Err(fetch_error) => {
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
