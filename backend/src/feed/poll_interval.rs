//! Adaptive poll interval calculations for the feed update queue.
//!
//! Extracted from the old in-process ingest pipeline (which fetched feeds
//! directly from the backend) when that pipeline was replaced by the
//! `worker` binary + `feed_update_queue` table. The worker doesn't decide
//! cadence itself -- it just reports whether a fetch happened and whether it
//! failed, and the backend (here) turns that into the next poll interval.

use chrono::{DateTime, Duration, Utc};

const MIN_POLL_INTERVAL_SECONDS: i64 = 60;

/// What a successful completion tells us about how often to poll the feed.
pub(crate) enum PollSignal {
    /// The feed body was fetched; poll at the observed item cadence.
    Fetched { cadence_seconds: Option<i64> },
    /// The feed was unchanged; back off from the current interval.
    NotModified { current_interval_seconds: i64 },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Schedule {
    pub(crate) poll_interval_seconds: i64,
    pub(crate) next_due_at: DateTime<Utc>,
}

pub(crate) fn reschedule_after_completion(
    signal: PollSignal,
    completed_at: DateTime<Utc>,
) -> Schedule {
    let poll_interval_seconds = match signal {
        PollSignal::Fetched { cadence_seconds } => resolved_poll_interval_seconds(cadence_seconds),
        PollSignal::NotModified {
            current_interval_seconds,
        } => backoff_poll_interval_seconds(current_interval_seconds),
    };
    Schedule {
        poll_interval_seconds,
        next_due_at: completed_at + Duration::seconds(poll_interval_seconds),
    }
}

/// A publisher's Retry-After can delay the next attempt but never shorten
/// our own backoff.
pub(crate) fn reschedule_after_failure(
    current_interval_seconds: i64,
    http_status: Option<u16>,
    retry_after: Option<DateTime<Utc>>,
    checked_at: DateTime<Utc>,
) -> Schedule {
    let poll_interval_seconds =
        failure_poll_interval_seconds(current_interval_seconds, http_status);
    let backoff_due_at = checked_at + Duration::seconds(poll_interval_seconds);
    Schedule {
        poll_interval_seconds,
        next_due_at: retry_after.map_or(backoff_due_at, |retry_after| {
            retry_after.max(backoff_due_at)
        }),
    }
}
const MAX_POLL_INTERVAL_SECONDS: i64 = 6_000;

/// Clamps an observed cadence (median seconds between items, see
/// `feed_item::read_feed_cadence_seconds`) into a sane poll interval. Falls
/// back to the slowest interval when there isn't yet a reliable cadence
/// signal, so newly-added feeds start out polled conservatively.
pub(crate) fn resolved_poll_interval_seconds(interval_seconds: Option<i64>) -> i64 {
    interval_seconds
        .unwrap_or(MAX_POLL_INTERVAL_SECONDS)
        .clamp(MIN_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS)
}

/// Doubles the current poll interval, clamped to the same bounds. Used both
/// when a feed reports "not modified" (it's not changing as often as we're
/// checking) and when a fetch fails (back off instead of hammering a broken
/// source).
pub(crate) fn backoff_poll_interval_seconds(current_interval_seconds: i64) -> i64 {
    (current_interval_seconds.saturating_mul(2))
        .clamp(MIN_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS)
}

/// Poll interval after a failed fetch. Feeds the publisher reports as gone
/// (HTTP 404/410) jump straight to the slowest interval instead of doubling,
/// since they are unlikely to come back soon.
pub(crate) fn failure_poll_interval_seconds(
    current_interval_seconds: i64,
    http_status: Option<u16>,
) -> i64 {
    match http_status {
        Some(404 | 410) => MAX_POLL_INTERVAL_SECONDS,
        _ => backoff_poll_interval_seconds(current_interval_seconds),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::UNIX_EPOCH + Duration::seconds(seconds)
    }

    #[test]
    fn test_reschedule_after_fetch_uses_clamped_cadence() {
        let completed_at = at(1_000);

        let schedule = reschedule_after_completion(
            PollSignal::Fetched {
                cadence_seconds: Some(30),
            },
            completed_at,
        );

        assert_eq!(
            schedule,
            Schedule {
                poll_interval_seconds: MIN_POLL_INTERVAL_SECONDS,
                next_due_at: completed_at + Duration::seconds(MIN_POLL_INTERVAL_SECONDS),
            }
        );
    }

    #[test]
    fn test_reschedule_after_not_modified_backs_off() {
        let completed_at = at(1_000);

        let schedule = reschedule_after_completion(
            PollSignal::NotModified {
                current_interval_seconds: 300,
            },
            completed_at,
        );

        assert_eq!(
            schedule,
            Schedule {
                poll_interval_seconds: 600,
                next_due_at: completed_at + Duration::seconds(600),
            }
        );
    }

    #[test]
    fn test_reschedule_after_failure_respects_retry_after_without_shortening_backoff() {
        let checked_at = at(1_000);
        let backoff_due_at = checked_at + Duration::seconds(600);
        let cases = [
            (None, backoff_due_at),
            (Some(checked_at - Duration::hours(1)), backoff_due_at),
            (Some(checked_at + Duration::seconds(60)), backoff_due_at),
            (
                Some(checked_at + Duration::hours(24)),
                checked_at + Duration::hours(24),
            ),
        ];

        for (retry_after, expected_due_at) in cases {
            let schedule = reschedule_after_failure(300, Some(503), retry_after, checked_at);

            assert_eq!(
                schedule,
                Schedule {
                    poll_interval_seconds: 600,
                    next_due_at: expected_due_at,
                }
            );
        }
    }

    #[test]
    fn test_failure_poll_interval_seconds_maxes_out_gone_feeds() {
        assert_eq!(
            failure_poll_interval_seconds(60, Some(404)),
            MAX_POLL_INTERVAL_SECONDS
        );
        assert_eq!(
            failure_poll_interval_seconds(60, Some(410)),
            MAX_POLL_INTERVAL_SECONDS
        );
    }

    #[test]
    fn test_failure_poll_interval_seconds_doubles_other_failures() {
        assert_eq!(failure_poll_interval_seconds(120, Some(503)), 240);
        assert_eq!(failure_poll_interval_seconds(120, None), 240);
    }

    quickcheck::quickcheck! {
        fn prop_resolved_poll_interval_seconds_clamps_db_value(interval_seconds: i64) -> bool {
            resolved_poll_interval_seconds(Some(interval_seconds))
                == interval_seconds.clamp(MIN_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS)
        }

        fn prop_resolved_poll_interval_seconds_uses_max_without_db_value(_input: bool) -> bool {
            resolved_poll_interval_seconds(None) == MAX_POLL_INTERVAL_SECONDS
        }

        fn prop_backoff_poll_interval_seconds_doubles_saturating_then_clamps(current_interval_seconds: i64) -> bool {
            let expected = current_interval_seconds
                .saturating_mul(2)
                .clamp(MIN_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS);

            backoff_poll_interval_seconds(current_interval_seconds) == expected
        }
    }
}
