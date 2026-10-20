use std::time::Duration;

use anyhow::Result;
use tokio::time::{MissedTickBehavior, interval, sleep};
use tracing::{info, warn};

use crate::http_util::send;

// PostgreSQL owns the hourly schedule; all workers poll for due work.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn run(backend_url: String, http: reqwest::Client) {
    let url = format!(
        "{}/internal/items/summary/refresh",
        backend_url.trim_end_matches('/')
    );
    let mut timer = interval(REFRESH_INTERVAL);
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        timer.tick().await;
        loop {
            info!("refreshing daily news summary");
            match refresh(&http, &url).await {
                Ok(()) => {
                    info!("daily news summary refresh accepted");
                    break;
                }
                Err(err) => {
                    warn!(
                        error = format!("{err:#}"),
                        "daily news summary refresh failed; retrying"
                    );
                    sleep(RETRY_INTERVAL).await;
                }
            }
        }
    }
}

async fn refresh(http: &reqwest::Client, url: &str) -> Result<()> {
    send(
        http.post(url).timeout(REQUEST_TIMEOUT),
        "refresh daily news summary",
    )
    .await?;
    Ok(())
}
