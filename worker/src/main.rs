mod cli;
mod embedding_indexer;
mod feed_fetcher;
mod feed_mapper;
mod feed_parser;
mod http_util;
mod queue_client;
mod runner;
mod summary_refresh;
mod telemetry;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Opts;
use feed_fetcher::{HttpFeedSource, http_client_builder};
use metrics::counter;
use metrics_exporter_prometheus::PrometheusBuilder;
use queue_client::QueueClient;
use runner::RunnerConfig;
use tokio::sync::watch;
use tokio::task::JoinSet;
use tokio::time::sleep;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const OLLAMA_TIMEOUT: Duration = Duration::from_secs(120);
const BACKGROUND_RESTART_DELAY: Duration = Duration::from_secs(5);

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(opts.log_level.as_str().to_ascii_lowercase()));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .flatten_event(true)
        .try_init()
        .map_err(|e| anyhow::anyhow!(e))
        .context("failed to install tracing subscriber")?;

    PrometheusBuilder::new()
        .with_http_listener(opts.metrics_host)
        .install()
        .context("failed to install metrics recorder/exporter")?;
    info!("Serving Prometheus metrics on {}", opts.metrics_host);

    let http = http_client_builder()
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;
    let queue = Arc::new(QueueClient::new(&opts.backend_url, http.clone()));
    let source = Arc::new(HttpFeedSource::new(http.clone()));
    let config = RunnerConfig {
        limit: opts.limit,
        lease_seconds: opts.lease_seconds,
        idle_sleep: Duration::from_secs(opts.idle_sleep_seconds),
    };

    if opts.once {
        runner::run_once(queue, source, config).await?;
        return Ok(());
    }

    let ollama_http = http_client_builder()
        .timeout(OLLAMA_TIMEOUT)
        .build()
        .context("failed to build Ollama HTTP client")?;
    let mut background = JoinSet::new();
    {
        let (backend_url, http) = (opts.backend_url.clone(), http.clone());
        background.spawn(supervise("summary_refresh", move || {
            summary_refresh::run(backend_url.clone(), http.clone())
        }));
    }
    {
        let (backend_url, ollama_url) = (opts.backend_url.clone(), opts.ollama_url.clone());
        background.spawn(supervise("embedding_indexer", move || {
            embedding_indexer::run(
                backend_url.clone(),
                ollama_url.clone(),
                http.clone(),
                ollama_http.clone(),
            )
        }));
    }

    let (stop, shutdown) = watch::channel(false);
    tokio::spawn(async move {
        match shutdown_signal().await {
            Ok(()) => info!("shutdown signal received"),
            Err(e) => error!(
                error = format!("{e:#}"),
                "failed to listen for shutdown signals"
            ),
        }
        let _ = stop.send(true);
    });

    runner::run(queue, source, config, shutdown).await;
    background.abort_all();
    info!("worker stopped");
    Ok(())
}

/// Runs a never-ending background task, restarting it if it panics or
/// returns, so one failing subsystem cannot silently disappear.
async fn supervise<F, Fut>(name: &'static str, mut make: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = ()> + Send + 'static,
{
    loop {
        match tokio::spawn(make()).await {
            Ok(()) => warn!(task = name, "background task exited; restarting"),
            Err(e) if e.is_cancelled() => return,
            Err(e) => error!(task = name, error = %e, "background task panicked; restarting"),
        }
        counter!("rss_centr_worker_background_task_restarts_total", "task" => name).increment(1);
        sleep(BACKGROUND_RESTART_DELAY).await;
    }
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut terminate =
            signal(SignalKind::terminate()).context("failed to install SIGTERM handler")?;
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("failed to listen for Ctrl-C"),
            _ = terminate.recv() => Ok(()),
        }
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c()
            .await
            .context("failed to listen for Ctrl-C")
    }
}
