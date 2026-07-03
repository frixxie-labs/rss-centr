use clap::Parser;
use tracing::Level;

#[derive(Debug, Clone)]
pub(crate) enum LogLevel {
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
pub(crate) struct Opts {
    #[arg(long, env = "BACKEND_URL", default_value = "http://localhost:8080")]
    pub(crate) backend_url: String,

    #[arg(long, default_value = "0.0.0.0:9090")]
    pub(crate) metrics_host: String,

    #[arg(long, default_value = "5")]
    pub(crate) limit: i64,

    #[arg(long, default_value = "300")]
    pub(crate) lease_seconds: i64,

    #[arg(long, default_value = "30")]
    pub(crate) idle_sleep_seconds: u64,

    #[arg(long)]
    pub(crate) once: bool,

    #[arg(short, long, default_value = "info")]
    pub(crate) log_level: LogLevel,
}
