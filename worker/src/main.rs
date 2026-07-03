mod cli;
mod feed_fetcher;
mod feed_mapper;
mod queue_client;
mod runner;
mod telemetry;

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use cli::Opts;
use metrics_exporter_prometheus::PrometheusBuilder;
use queue_client::QueueClient;
use runner::run_once;
use tokio::time::sleep;
use tracing::{Level, info, warn};
use tracing_subscriber::FmtSubscriber;

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
