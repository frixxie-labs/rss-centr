use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use metrics_exporter_prometheus::PrometheusBuilder;
use reqwest::Client;
use sqlx::PgPool;
use tokio::{net::TcpListener, sync::broadcast};
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;

use crate::{
    config::BackendConfig,
    events::{NewFeedItemEvent, NewFeedItemListener},
    handlers::create_router,
    search::SearchState,
    summary::SummaryState,
};

mod config;
pub mod events;
pub mod feed;
pub mod handlers;
pub mod search;
mod summary;

const SEARCH_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

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

#[derive(Debug, Clone, Parser)]
pub struct Opts {
    #[arg(long, default_value = "0.0.0.0:8080")]
    host: String,

    #[arg(
        short,
        long,
        env = "DATABASE_URL",
        default_value = "postgres://postgres:postgres@localhost:5432/rss_centr"
    )]
    db_url: String,

    #[arg(short, long, default_value = "info")]
    log_level: LogLevel,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let config = BackendConfig::from_lookup(|name| std::env::var(name).ok());
    let level: Level = opts.log_level.into();
    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .json()
        .flatten_event(true)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("failed to install tracing subscriber")?;

    let search_http = Client::builder()
        .timeout(SEARCH_HTTP_TIMEOUT)
        .build()
        .context("failed to build search HTTP client")?;
    // Summary requests set their own metadata/generation deadlines; do not apply
    // the search client's overall timeout to these longer-running calls.
    let summary_http = Client::builder()
        .build()
        .context("failed to build summary HTTP client")?;

    let metrics_handler = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install metrics recorder/exporter")?;

    let redacted_db_url = redact_db_url(&opts.db_url);
    info!("Connecting to DB at {redacted_db_url}");
    let pool = PgPool::connect(&opts.db_url)
        .await
        .with_context(|| format!("failed to connect to {redacted_db_url}"))?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    let (new_item_tx, _new_item_rx) = broadcast::channel::<NewFeedItemEvent>(1 << 12);
    let new_item_listener = NewFeedItemListener::connect(&pool)
        .await
        .context("failed to start new feed item listener")?;

    let new_item_listener_pool = pool.clone();
    let new_item_listener_tx = new_item_tx.clone();
    let new_item_listener_handle = tokio::spawn(async move {
        new_item_listener
            .run(new_item_listener_pool, new_item_listener_tx)
            .await
    });

    let search_state = SearchState::new(
        pool.clone(),
        search_http,
        config.ollama_url.clone(),
        config.search,
    );
    let summary_state = SummaryState::new(
        pool.clone(),
        summary_http,
        config.ollama_url,
        config.summary_models,
    );
    let centroids = search_state.centroids.clone();
    let centroid_pool = pool.clone();
    tokio::spawn(async move { centroids.run_loader(&centroid_pool).await });

    let app = create_router(
        pool,
        search_state,
        summary_state,
        metrics_handler,
        new_item_tx,
    );

    let listener = TcpListener::bind(&opts.host).await?;
    tokio::select! {
        result = axum::serve(listener, app) => {
            result.context("server exited unexpectedly")?;
        }
        result = new_item_listener_handle => {
            match result {
                Ok(Ok(())) => error!("new item listener exited unexpectedly"),
                Ok(Err(e)) => error!("new item listener failed: {e:#}"),
                Err(e) => error!("new item listener panicked: {e}"),
            }
            anyhow::bail!("new item listener stopped, shutting down");
        }
    }
    Ok(())
}

/// Hide the password of a database URL so it can be logged.
fn redact_db_url(db_url: &str) -> String {
    match reqwest::Url::parse(db_url) {
        Ok(mut url) => {
            if url.password().is_some() && url.set_password(Some("***")).is_err() {
                return "<unparseable database url>".to_string();
            }
            url.to_string()
        }
        Err(_) => "<unparseable database url>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::redact_db_url;

    #[test]
    fn redacts_password() {
        assert_eq!(
            redact_db_url("postgresql://postgres:admin@server:5432/rsscentr"),
            "postgresql://postgres:***@server:5432/rsscentr"
        );
    }

    #[test]
    fn keeps_url_without_password() {
        assert_eq!(
            redact_db_url("postgres://postgres@localhost:5432/rss_centr"),
            "postgres://postgres@localhost:5432/rss_centr"
        );
    }

    #[test]
    fn hides_unparseable_url() {
        assert_eq!(
            redact_db_url("not a url :secret"),
            "<unparseable database url>"
        );
    }
}
