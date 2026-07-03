use std::time::Duration;

use metrics::{counter, histogram};

/// Duration of a single `fetch_feed` call (network request + parse),
/// labeled by outcome. Mirrors the old backend-side
/// `rss_centr_feed_source_fetch_duration_seconds` metric, which measured the
/// same thing before fetching moved from the backend's ingest pipeline into
/// this worker.
pub(crate) fn record_feed_fetch_duration(outcome: &str, elapsed: Duration) {
    let labels = [("outcome", outcome.to_string())];
    histogram!("rss_centr_worker_feed_fetch_duration_seconds", &labels).record(elapsed);
}

/// Count of feeds this worker finished processing, labeled by outcome:
/// "fetched" (new content), "not_modified" (304/cache-validated unchanged),
/// or "failed" (fetch error, rescheduled with backoff).
pub(crate) fn record_feed_processed(outcome: &str) {
    let labels = [("outcome", outcome.to_string())];
    counter!("rss_centr_worker_feeds_processed_total", &labels).increment(1);
}
