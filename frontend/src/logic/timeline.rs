use crate::models::{FeedItem, ReplayDone};

use super::merge_items;

pub fn resume_cursor(items: &[FeedItem], stored: Option<String>) -> Option<String> {
    items
        .iter()
        .map(|item| item.id)
        .max()
        .map(|id| id.to_string())
        .or(stored)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiveOutcome {
    Buffered,
    Live,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayOutcome {
    NoRefresh,
    Refresh,
}

#[derive(Clone, Debug, PartialEq)]
enum TimelinePhase {
    Replaying {
        buffer: Vec<FeedItem>,
        cursor: Option<String>,
    },
    Live,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineState {
    pub items: Vec<FeedItem>,
    pub limited: bool,
    /// Last committed cursor; buffered events do not advance it until replay ends.
    pub cursor: Option<String>,
    phase: TimelinePhase,
}

impl TimelineState {
    pub fn new(items: Vec<FeedItem>) -> Self {
        let mut state = Self {
            items: Vec::new(),
            limited: false,
            cursor: None,
            phase: TimelinePhase::Replaying {
                buffer: Vec::new(),
                cursor: None,
            },
        };
        merge_items(&mut state.items, items);
        state
    }

    pub fn is_replaying(&self) -> bool {
        matches!(&self.phase, TimelinePhase::Replaying { .. })
    }

    /// Repeated connection errors must not discard an unfinished replay.
    pub fn begin_replay(&mut self) {
        if !self.is_replaying() {
            self.phase = TimelinePhase::Replaying {
                buffer: Vec::new(),
                cursor: None,
            };
        }
    }

    pub fn receive(&mut self, item: FeedItem, event_id: String) -> ReceiveOutcome {
        match &mut self.phase {
            TimelinePhase::Replaying { buffer, cursor } => {
                buffer.push(item);
                *cursor = Some(event_id);
                ReceiveOutcome::Buffered
            }
            TimelinePhase::Live => {
                merge_items(&mut self.items, [item]);
                self.cursor = Some(event_id);
                ReceiveOutcome::Live
            }
        }
    }

    pub fn finish_replay(&mut self, marker: ReplayDone) -> ReplayOutcome {
        let (buffer, replay_cursor) = match std::mem::replace(&mut self.phase, TimelinePhase::Live)
        {
            TimelinePhase::Replaying { buffer, cursor } => (buffer, cursor),
            TimelinePhase::Live => (Vec::new(), None),
        };
        let outcome = if !buffer.is_empty() || marker.replayed > 0 {
            ReplayOutcome::Refresh
        } else {
            ReplayOutcome::NoRefresh
        };
        merge_items(&mut self.items, buffer);
        self.limited = marker.limited;
        self.cursor = marker
            .last_event_id
            .map(|id| id.to_string())
            .or(replay_cursor)
            .or(self.cursor.take());
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64) -> FeedItem {
        FeedItem {
            id,
            inserted_at: "2025-01-01T00:00:00Z".parse().unwrap(),
            ..Default::default()
        }
    }

    fn ids(items: &[FeedItem]) -> Vec<i64> {
        items.iter().map(|item| item.id).collect()
    }

    #[test]
    fn replay_batches_deduplicate_and_replace_details() {
        let mut state = TimelineState::new((1..=501).map(item).collect());
        assert!(state.is_replaying());
        assert_eq!(state.items.len(), 501);
        assert_eq!(state.items[0].id, 501);
        let mut updated = item(501);
        updated.title = "Updated".into();
        assert_eq!(
            state.receive(updated, "501".into()),
            ReceiveOutcome::Buffered
        );
        assert_eq!(
            state.receive(item(502), "502".into()),
            ReceiveOutcome::Buffered
        );
        assert_eq!(state.items[0].id, 501);
        assert_eq!(
            state.finish_replay(ReplayDone {
                replayed: 2,
                limited: true,
                last_event_id: Some(502)
            }),
            ReplayOutcome::Refresh
        );
        assert_eq!(state.items.len(), 502);
        assert_eq!(state.items[0].id, 502);
        assert_eq!(state.items[1].title, "Updated");
        assert_eq!(state.cursor.as_deref(), Some("502"));
        assert!(state.limited);
        state.begin_replay();
        state.receive(item(503), "503".into());
        assert_eq!(
            state.finish_replay(ReplayDone::default()),
            ReplayOutcome::Refresh
        );
        assert_eq!(state.cursor.as_deref(), Some("503"));
    }

    #[test]
    fn live_items_change_the_visible_list_immediately() {
        let mut state = TimelineState::new(vec![item(1)]);
        state.finish_replay(ReplayDone::default());
        assert_eq!(state.receive(item(2), "2".into()), ReceiveOutcome::Live);
        assert_eq!(ids(&state.items), [2, 1]);
        assert_eq!(state.cursor.as_deref(), Some("2"));
    }

    #[test]
    fn repeated_stream_errors_keep_buffered_items_and_the_committed_cursor() {
        let mut state = TimelineState::new(vec![item(1)]);
        state.cursor = Some("1".into());
        state.receive(item(2), "2".into());
        state.begin_replay();
        state.receive(item(3), "3".into());
        state.begin_replay();
        state.receive(item(4), "4".into());
        assert_eq!(ids(&state.items), [1]);
        assert_eq!(state.cursor.as_deref(), Some("1"));

        assert_eq!(
            state.finish_replay(ReplayDone::default()),
            ReplayOutcome::Refresh
        );

        assert_eq!(ids(&state.items), [4, 3, 2, 1]);
        assert_eq!(state.cursor.as_deref(), Some("4"));
        assert!(!state.is_replaying());
    }

    #[test]
    fn replay_cursor_precedence_is_marker_then_last_buffered_event_then_committed_cursor() {
        for (buffered, marker_id, expected) in [
            (true, Some(20), "20"),
            (true, None, "15"),
            (false, Some(20), "20"),
            (false, None, "10"),
        ] {
            let mut state = TimelineState::new(vec![item(10)]);
            state.cursor = Some("10".into());
            if buffered {
                state.receive(item(15), "15".into());
            }
            let outcome = state.finish_replay(ReplayDone {
                last_event_id: marker_id,
                ..Default::default()
            });
            assert_eq!(
                outcome,
                if buffered {
                    ReplayOutcome::Refresh
                } else {
                    ReplayOutcome::NoRefresh
                }
            );
            assert_eq!(state.cursor.as_deref(), Some(expected));
            assert!(!state.is_replaying());
        }
    }

    #[test]
    fn replay_markers_can_request_refresh_without_buffered_items_and_reset_the_limit_notice() {
        let mut state = TimelineState::new(vec![item(1)]);
        state.cursor = Some("1".into());
        assert_eq!(
            state.finish_replay(ReplayDone {
                replayed: 2,
                limited: true,
                last_event_id: Some(3)
            }),
            ReplayOutcome::Refresh
        );
        assert!(state.limited);
        assert_eq!(state.cursor.as_deref(), Some("3"));

        state.begin_replay();
        assert_eq!(
            state.finish_replay(ReplayDone::default()),
            ReplayOutcome::NoRefresh
        );

        assert!(!state.limited);
        assert_eq!(state.cursor.as_deref(), Some("3"));
        assert_eq!(ids(&state.items), [1]);
    }

    #[test]
    fn replay_markers_received_while_live_keep_items_and_apply_the_marker_cursor() {
        let mut state = TimelineState::new(vec![item(1)]);
        state.finish_replay(ReplayDone::default());
        state.receive(item(2), "2".into());

        let outcome = state.finish_replay(ReplayDone {
            replayed: 1,
            limited: true,
            last_event_id: Some(3),
        });

        assert_eq!(outcome, ReplayOutcome::Refresh);
        assert_eq!(ids(&state.items), [2, 1]);
        assert_eq!(state.cursor.as_deref(), Some("3"));
        assert!(state.limited);
        assert!(!state.is_replaying());
        assert_eq!(
            state.finish_replay(ReplayDone::default()),
            ReplayOutcome::NoRefresh
        );
        assert_eq!(state.cursor.as_deref(), Some("3"));
        assert!(!state.limited);
    }

    #[test]
    fn resume_prefers_initial_items_over_stored_cursor() {
        assert_eq!(
            resume_cursor(&[item(2), item(5)], Some("999".into())),
            Some("5".into())
        );
        assert_eq!(resume_cursor(&[], Some("999".into())), Some("999".into()));
        assert_eq!(resume_cursor(&[], None), None);
    }
}
