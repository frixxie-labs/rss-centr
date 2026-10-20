use std::net::SocketAddr;

use clap::Parser;
use tracing::Level;

#[derive(Debug, Parser)]
pub(crate) struct Opts {
    #[arg(long, env = "BACKEND_URL", default_value = "http://localhost:8080")]
    pub(crate) backend_url: String,

    /// Ollama base URL used to embed feed items for semantic search.
    #[arg(long, env = "OLLAMA_URL", default_value = "http://desktop:11434")]
    pub(crate) ollama_url: String,

    /// Prometheus metrics listen address.
    #[arg(long, env = "METRICS_HOST", default_value = "0.0.0.0:9090")]
    pub(crate) metrics_host: SocketAddr,

    /// Maximum number of feeds fetched concurrently.
    #[arg(
        long,
        env = "WORKER_LIMIT",
        default_value = "5",
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub(crate) limit: u32,

    /// How long a dequeued feed stays leased to this worker.
    #[arg(
        long,
        env = "WORKER_LEASE_SECONDS",
        default_value = "300",
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    pub(crate) lease_seconds: u32,

    /// Seconds to wait before polling again when the queue is empty.
    #[arg(long, env = "WORKER_IDLE_SLEEP_SECONDS", default_value = "30")]
    pub(crate) idle_sleep_seconds: u64,

    /// Process one batch of feeds and exit. The embedding indexer and
    /// summary refresh do not run in this mode.
    #[arg(long)]
    pub(crate) once: bool,

    /// Log level (trace, debug, info, warn, error). `RUST_LOG` overrides it.
    #[arg(short, long, default_value = "info")]
    pub(crate) log_level: Level,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let opts = Opts::try_parse_from(["worker"]).unwrap();
        assert_eq!(opts.limit, 5);
        assert_eq!(opts.lease_seconds, 300);
        assert_eq!(opts.metrics_host, "0.0.0.0:9090".parse().unwrap());
        assert_eq!(opts.log_level, Level::INFO);
    }

    #[test]
    fn test_rejects_zero_limit_and_lease() {
        assert!(Opts::try_parse_from(["worker", "--limit", "0"]).is_err());
        assert!(Opts::try_parse_from(["worker", "--lease-seconds", "0"]).is_err());
    }

    #[test]
    fn test_rejects_invalid_metrics_host_and_log_level() {
        assert!(Opts::try_parse_from(["worker", "--metrics-host", "nope"]).is_err());
        assert!(Opts::try_parse_from(["worker", "--log-level", "loud"]).is_err());
    }
}
