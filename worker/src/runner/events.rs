use chrono::{DateTime, Utc};
use metrics::counter;
use rillfp::{Logger, Writer};
use rss_centr_core::feed_update_queue::{CompleteFeedUpdateResult, CompletionKind};
use tracing::info;

use crate::telemetry::record_feed_processed;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProcessingOutcome {
    Fetched,
    NotModified,
    Failed,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum FeedEvent {
    Processed(ProcessingOutcome),
    ItemsInserted(u64),
    Completed {
        feed_id: i64,
        kind: CompletionKind,
        fetched_items: usize,
        inserted_items: i64,
        next_due_at: DateTime<Utc>,
    },
}

/// Decide which metrics and log entry follow an acknowledged completion.
pub(super) fn completed(
    feed_id: i64,
    kind: CompletionKind,
    fetched_items: usize,
    result: CompleteFeedUpdateResult,
) -> Writer<(), FeedEvent> {
    let outcome = match kind {
        CompletionKind::Fetched => ProcessingOutcome::Fetched,
        CompletionKind::NotModified => ProcessingOutcome::NotModified,
    };
    let mut events = vec![FeedEvent::Processed(outcome)];
    if kind == CompletionKind::Fetched {
        events.push(FeedEvent::ItemsInserted(result.inserted_items as u64));
    }
    events.push(FeedEvent::Completed {
        feed_id,
        kind,
        fetched_items,
        inserted_items: result.inserted_items,
        next_due_at: result.next_due_at,
    });
    Writer::new((), events)
}

pub(super) struct FeedTelemetry;

impl Logger<FeedEvent> for FeedTelemetry {
    fn log(&self, event: FeedEvent) {
        match event {
            FeedEvent::Processed(outcome) => record_feed_processed(match outcome {
                ProcessingOutcome::Fetched => "fetched",
                ProcessingOutcome::NotModified => "not_modified",
                ProcessingOutcome::Failed => "failed",
            }),
            FeedEvent::ItemsInserted(count) => {
                counter!("rss_centr_worker_feed_items_inserted_total").increment(count);
            }
            FeedEvent::Completed {
                feed_id,
                kind,
                fetched_items,
                inserted_items,
                next_due_at,
            } => match kind {
                CompletionKind::Fetched => info!(
                    target: "worker::runner",
                    feed_id,
                    fetched_items,
                    inserted_items,
                    %next_due_at,
                    "feed update completed"
                ),
                CompletionKind::NotModified => info!(
                    target: "worker::runner",
                    feed_id,
                    inserted_items,
                    %next_due_at,
                    "feed not modified"
                ),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use chrono::DateTime;

    use super::*;

    #[test]
    fn completion_events_are_deferred_and_keep_metric_then_log_order() {
        let next_due_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        for (kind, outcome) in [
            (CompletionKind::Fetched, ProcessingOutcome::Fetched),
            (CompletionKind::NotModified, ProcessingOutcome::NotModified),
        ] {
            let received = RefCell::new(Vec::new());
            let writer = completed(
                42,
                kind,
                7,
                CompleteFeedUpdateResult {
                    inserted_items: 3,
                    next_due_at,
                },
            );
            assert!(received.borrow().is_empty());
            writer.write_to(&|event| received.borrow_mut().push(event));

            let mut expected = vec![FeedEvent::Processed(outcome)];
            if kind == CompletionKind::Fetched {
                expected.push(FeedEvent::ItemsInserted(3));
            }
            expected.push(FeedEvent::Completed {
                feed_id: 42,
                kind,
                fetched_items: 7,
                inserted_items: 3,
                next_due_at,
            });
            assert_eq!(*received.borrow(), expected);
        }
    }

    quickcheck::quickcheck! {
        fn prop_fetched_completion_preserves_counts(feed_id: i64, fetched: u16, inserted: u16) -> bool {
            let next_due_at = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
            let (_, events) = completed(feed_id, CompletionKind::Fetched, usize::from(fetched), CompleteFeedUpdateResult {
                inserted_items: i64::from(inserted), next_due_at,
            }).into_parts();
            events == [
                FeedEvent::Processed(ProcessingOutcome::Fetched),
                FeedEvent::ItemsInserted(u64::from(inserted)),
                FeedEvent::Completed { feed_id, kind: CompletionKind::Fetched, fetched_items: usize::from(fetched), inserted_items: i64::from(inserted), next_due_at },
            ]
        }
    }
}
