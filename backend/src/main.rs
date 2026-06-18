use anyhow::{Context, Result};
use clap::Parser;
use metrics_exporter_prometheus::PrometheusBuilder;
use sqlx::PgPool;
use std::{collections::HashSet, sync::Arc, time::Duration};
use tokio::{
    net::TcpListener,
    sync::{Mutex, broadcast, mpsc::channel},
};
use tracing::{Level, error, info};
use tracing_subscriber::FmtSubscriber;

use crate::{
    background_tasks::{IngestJob, enqueue_due_feeds_loop, handle_ingest_bg_thread},
    events::{NewFeedItemEvent, NewFeedItemListener},
    handlers::create_router,
};

pub mod background_tasks;
pub mod events;
pub mod feed;
pub mod handlers;

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

    #[arg(long, default_value = "30")]
    scheduler_interval_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let level: Level = opts.log_level.into();
    let subscriber = FmtSubscriber::builder().with_max_level(level).finish();
    tracing::subscriber::set_global_default(subscriber)
        .context("failed to install tracing subscriber")?;

    let metrics_handler = PrometheusBuilder::new()
        .install_recorder()
        .context("failed to install metrics recorder/exporter")?;

    info!("Connecting to DB at {}", opts.db_url);
    let pool = PgPool::connect(&opts.db_url)
        .await
        .with_context(|| format!("failed to connect to {}", opts.db_url))?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("failed to run migrations")?;

    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(20))
        .build()
        .context("failed to build HTTP client")?;
    let (tx, rx) = channel::<IngestJob>(1 << 12);
    let (new_item_tx, _new_item_rx) = broadcast::channel::<NewFeedItemEvent>(1 << 12);
    let new_item_listener = NewFeedItemListener::connect(&pool)
        .await
        .context("failed to start new feed item listener")?;
    let in_flight = Arc::new(Mutex::new(HashSet::<i64>::new()));

    let ingest_pool = pool.clone();
    let ingest_client = client.clone();
    let ingest_in_flight = in_flight.clone();
    let ingest_handle = tokio::spawn(async move {
        handle_ingest_bg_thread(rx, ingest_pool, ingest_client, ingest_in_flight).await;
    });

    let new_item_listener_pool = pool.clone();
    let new_item_listener_tx = new_item_tx.clone();
    let new_item_listener_handle = tokio::spawn(async move {
        new_item_listener
            .run(new_item_listener_pool, new_item_listener_tx)
            .await
    });

    let sched_pool = pool.clone();
    let sched_tx = tx.clone();
    let sched_in_flight = in_flight.clone();
    let every = std::time::Duration::from_secs(opts.scheduler_interval_seconds);
    let scheduler_handle = tokio::spawn(async move {
        enqueue_due_feeds_loop(sched_pool, sched_tx, every, sched_in_flight).await
    });

    let app = create_router(pool, metrics_handler, tx, new_item_tx);

    let listener = TcpListener::bind(&opts.host).await?;
    tokio::select! {
        result = axum::serve(listener, app) => {
            result.context("server exited unexpectedly")?;
        }
        result = ingest_handle => {
            match result {
                Ok(()) => error!("ingest worker exited unexpectedly"),
                Err(e) => error!("ingest worker panicked: {e}"),
            }
            anyhow::bail!("ingest worker stopped, shutting down");
        }
        result = scheduler_handle => {
            match result {
                Ok(Ok(())) => error!("scheduler exited unexpectedly"),
                Ok(Err(e)) => error!("scheduler failed: {e:#}"),
                Err(e) => error!("scheduler panicked: {e}"),
            }
            anyhow::bail!("scheduler stopped, shutting down");
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
