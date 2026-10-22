use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use rss_centr_core::summary_refresh::SummaryRefreshResponse;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::http_util::send;

// PostgreSQL owns the hourly schedule; workers sleep until the backend says
// the next refresh is due, bounded so missed schedule changes are picked up.
const MIN_REFRESH_DELAY: Duration = Duration::from_secs(60);
const MAX_REFRESH_DELAY: Duration = Duration::from_secs(15 * 60);
const FALLBACK_REFRESH_DELAY: Duration = Duration::from_secs(10 * 60);
const RETRY_INTERVAL: Duration = Duration::from_secs(60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) async fn run(backend_url: String, http: reqwest::Client) {
    let url = format!(
        "{}/internal/items/summary/refresh",
        backend_url.trim_end_matches('/')
    );

    loop {
        let delay = match refresh(&http, &url).await {
            Ok(next_due_at) => {
                let delay = next_refresh_delay(next_due_at, Utc::now());
                info!(
                    next_due_at = ?next_due_at,
                    delay_seconds = delay.as_secs(),
                    "daily news summary refresh accepted"
                );
                delay
            }
            Err(err) => {
                warn!(
                    error = format!("{err:#}"),
                    "daily news summary refresh failed; retrying"
                );
                RETRY_INTERVAL
            }
        };
        sleep(delay).await;
    }
}

/// How long to wait before asking the backend again.
fn next_refresh_delay(next_due_at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> Duration {
    next_due_at.map_or(FALLBACK_REFRESH_DELAY, |due| {
        (due - now)
            .to_std()
            .unwrap_or(Duration::ZERO)
            .clamp(MIN_REFRESH_DELAY, MAX_REFRESH_DELAY)
    })
}

/// Returns the next due time, or `None` if the backend did not report one.
async fn refresh(http: &reqwest::Client, url: &str) -> Result<Option<DateTime<Utc>>> {
    let response = send(
        http.post(url).timeout(REQUEST_TIMEOUT),
        "refresh daily news summary",
    )
    .await?;
    Ok(response
        .json::<SummaryRefreshResponse>()
        .await
        .ok()
        .map(|body| body.next_due_at))
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    #[test]
    fn waits_until_due() {
        let due = now() + TimeDelta::minutes(5);
        assert_eq!(
            next_refresh_delay(Some(due), now()),
            Duration::from_secs(300)
        );
    }

    #[test]
    fn waits_at_least_the_minimum_when_already_due() {
        let due = now() - TimeDelta::minutes(5);
        assert_eq!(next_refresh_delay(Some(due), now()), MIN_REFRESH_DELAY);
        assert_eq!(next_refresh_delay(Some(now()), now()), MIN_REFRESH_DELAY);
    }

    #[test]
    fn caps_long_waits() {
        let due = now() + TimeDelta::hours(1);
        assert_eq!(next_refresh_delay(Some(due), now()), MAX_REFRESH_DELAY);
    }

    #[test]
    fn falls_back_without_due_time() {
        assert_eq!(next_refresh_delay(None, now()), FALLBACK_REFRESH_DELAY);
    }
}
