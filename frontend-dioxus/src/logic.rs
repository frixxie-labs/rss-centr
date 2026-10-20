use std::collections::BTreeMap;

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use crate::models::{FeedItem, ReplayDone};

pub const PAGE_SIZE: usize = 50;
pub const CURSOR_KEY: &str = "rss:last-event-id";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewsFilters {
    pub query: String,
    pub feed_id: Option<i64>,
    pub mode: String,
}

impl Default for NewsFilters {
    fn default() -> Self {
        Self {
            query: String::new(),
            feed_id: None,
            mode: "hybrid".into(),
        }
    }
}

impl NewsFilters {
    pub fn parse(query: &str) -> Self {
        let params: BTreeMap<_, _> =
            form_urlencoded::parse(query.trim_start_matches('?').as_bytes())
                .into_owned()
                .collect();
        Self {
            query: params
                .get("q")
                .map(|s| s.trim().to_owned())
                .unwrap_or_default(),
            feed_id: params
                .get("source_id")
                .or_else(|| params.get("feed_id"))
                .and_then(|s| s.parse::<i64>().ok())
                .filter(|id| *id > 0),
            mode: params
                .get("mode")
                .filter(|m| matches!(m.as_str(), "hybrid" | "semantic" | "keyword"))
                .cloned()
                .unwrap_or_else(|| "hybrid".into()),
        }
    }

    pub fn api_query(&self, offset: usize) -> String {
        let mut params = form_urlencoded::Serializer::new(String::new());
        params.append_pair("limit", &PAGE_SIZE.to_string());
        if offset > 0 {
            params.append_pair("offset", &offset.to_string());
        }
        if let Some(id) = self.feed_id {
            params.append_pair("feed_id", &id.to_string());
        }
        if !self.query.trim().is_empty() {
            params.append_pair("q", self.query.trim());
            params.append_pair("mode", &self.mode);
        }
        params.finish()
    }
}

pub fn topic_url(word: &str, feed_id: i64) -> String {
    let query = form_urlencoded::Serializer::new(String::new())
        .append_pair("q", word)
        .append_pair("source_id", &feed_id.to_string())
        .finish();
    format!("/news?{query}")
}

pub fn timestamp(value: &str) -> i64 {
    DateTime::parse_from_rfc3339(value)
        .map(|d| d.timestamp_millis())
        .unwrap_or_default()
}

pub fn effective_date(item: &FeedItem) -> i64 {
    let inserted = timestamp(&item.inserted_at);
    item.published_at
        .as_deref()
        .map(timestamp)
        .unwrap_or(inserted)
        .min(inserted)
}

pub fn sort_newest(mut items: Vec<FeedItem>) -> Vec<FeedItem> {
    items.sort_by_key(|item| std::cmp::Reverse((effective_date(item), item.id)));
    items
}

pub fn display_order(items: Vec<FeedItem>, mode: &str, query: &str) -> Vec<FeedItem> {
    if mode != "keyword" && !query.trim().is_empty() {
        items
    } else {
        sort_newest(items)
    }
}

pub fn merge_items(items: &mut Vec<FeedItem>, incoming: impl IntoIterator<Item = FeedItem>) {
    let mut by_id: BTreeMap<_, _> = items.drain(..).map(|i| (i.id, i)).collect();
    by_id.extend(incoming.into_iter().map(|i| (i.id, i)));
    *items = sort_newest(by_id.into_values().collect());
}

/// Adds the next page to `items` and returns whether more pages may follow.
pub fn append_page(
    items: &mut Vec<FeedItem>,
    page: Vec<FeedItem>,
    mode: &str,
    query: &str,
) -> bool {
    let has_more = page.len() >= PAGE_SIZE;
    if mode != "keyword" && !query.trim().is_empty() {
        let seen: std::collections::HashSet<i64> = items.iter().map(|i| i.id).collect();
        items.extend(page.into_iter().filter(|i| !seen.contains(&i.id)));
    } else {
        merge_items(items, page);
    }
    has_more
}

pub fn resume_cursor(items: &[FeedItem], stored: Option<String>) -> Option<String> {
    items
        .iter()
        .map(|i| i.id)
        .max()
        .map(|id| id.to_string())
        .or(stored)
}

#[derive(Clone, Debug, PartialEq)]
pub struct TimelineState {
    pub items: Vec<FeedItem>,
    pub replaying: bool,
    pub limited: bool,
    pub cursor: Option<String>,
    buffer: Vec<FeedItem>,
    replay_cursor: Option<String>,
}

impl TimelineState {
    pub fn new(items: Vec<FeedItem>) -> Self {
        let mut state = Self {
            items: vec![],
            replaying: true,
            limited: false,
            cursor: None,
            buffer: vec![],
            replay_cursor: None,
        };
        merge_items(&mut state.items, items);
        state
    }

    pub fn receive(&mut self, item: FeedItem, event_id: String) {
        if self.replaying {
            self.buffer.push(item);
            self.replay_cursor = Some(event_id);
        } else {
            merge_items(&mut self.items, [item]);
            self.cursor = Some(event_id);
        }
    }

    pub fn finish_replay(&mut self, marker: ReplayDone) -> bool {
        let had_items = !self.buffer.is_empty() || marker.replayed > 0;
        merge_items(&mut self.items, self.buffer.drain(..));
        self.replaying = false;
        self.limited = marker.limited;
        self.cursor = marker
            .last_event_id
            .map(|id| id.to_string())
            .or(self.replay_cursor.take())
            .or(self.cursor.take());
        had_items
    }
}

pub fn compact_text(value: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for ch in value.chars() {
        match ch {
            '<' => {
                in_tag = true;
                text.push(' ');
            }
            '>' if in_tag => {
                in_tag = false;
                text.push(' ');
            }
            _ if !in_tag => text.push(ch),
            _ => {}
        }
    }
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn preview(item: &FeedItem) -> String {
    [item.summary.as_deref(), item.content.as_deref()]
        .into_iter()
        .flatten()
        .map(compact_text)
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

pub const PREVIEW_CHARS: usize = 240;

/// Drops the full article body so list payloads only carry what a card renders.
pub fn slim_item(mut item: FeedItem) -> FeedItem {
    let text = truncate(&preview(&item), PREVIEW_CHARS);
    item.summary = (!text.is_empty()).then_some(text);
    item.content = None;
    item
}

pub fn truncate(value: &str, max: usize) -> String {
    if value.chars().count() <= max {
        return value.into();
    }
    format!(
        "{}…",
        value
            .chars()
            .take(max.saturating_sub(1))
            .collect::<String>()
            .trim_end()
    )
}

pub fn hostname(value: &str) -> String {
    url::Url::parse(value)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h.trim_start_matches("www.").to_owned())
        })
        .unwrap_or_default()
}

pub fn time_ago(value: &str, now: i64) -> String {
    let seconds = (now - timestamp(value)) / 1000;
    match seconds {
        ..60 => "just now".into(),
        60..3600 => format!("{}m ago", seconds / 60),
        3600..86400 => format!("{}h ago", seconds / 3600),
        86400..2592000 => format!("{}d ago", seconds / 86400),
        _ => absolute_date(Some(value)),
    }
}

// Explicit UTC formatting is stable across server and browser during hydration.
pub fn absolute_date(value: Option<&str>) -> String {
    match value {
        None => "Never".into(),
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|d| {
                d.with_timezone(&chrono::Utc)
                    .format("%Y-%m-%d %H:%M UTC")
                    .to_string()
            })
            .unwrap_or_else(|_| "Unknown".into()),
    }
}

pub fn poll_interval(seconds: i64) -> String {
    match (seconds / 3600, seconds % 3600 / 60, seconds % 60) {
        (h, m, _) if h > 0 && m > 0 => format!("{h}h {m}m"),
        (h, _, _) if h > 0 => format!("{h}h"),
        (_, m, s) if m > 0 && s > 0 => format!("{m}m {s}s"),
        (_, m, _) if m > 0 => format!("{m}m"),
        (_, _, s) => format!("{s}s"),
    }
}
