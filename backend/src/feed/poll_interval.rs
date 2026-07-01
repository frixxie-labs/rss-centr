//! Adaptive poll interval calculations for the feed update queue.
//!
//! Extracted from the old in-process ingest pipeline (which fetched feeds
//! directly from the backend) when that pipeline was replaced by the
//! `worker` binary + `feed_update_queue` table. The worker doesn't decide
//! cadence itself -- it just reports whether a fetch happened and whether it
//! failed, and the backend (here) turns that into the next poll interval.

const MIN_POLL_INTERVAL_SECONDS: i64 = 60;
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

#[cfg(test)]
mod tests {
    use super::*;

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
