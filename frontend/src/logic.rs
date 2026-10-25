mod timeline;

pub use timeline::{ReceiveOutcome, ReplayOutcome, TimelineState, resume_cursor};

use std::collections::BTreeMap;

use chrono::DateTime;
use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use crate::models::{Feed, FeedItem};

pub const PAGE_SIZE: usize = 50;
pub const CURSOR_KEY: &str = "rss:last-event-id";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    #[default]
    Hybrid,
    Semantic,
    Keyword,
}

impl SearchMode {
    pub const ALL: [Self; 3] = [Self::Hybrid, Self::Semantic, Self::Keyword];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hybrid => "hybrid",
            Self::Semantic => "semantic",
            Self::Keyword => "keyword",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Hybrid => "Hybrid",
            Self::Semantic => "Semantic",
            Self::Keyword => "Keyword",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.as_str() == value)
    }
}

/// How a list of items is ordered on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    Newest,
    /// Keeps the backend's relevance order.
    Ranked,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct NewsFilters {
    pub query: String,
    pub feed_id: Option<i64>,
    pub mode: SearchMode,
}

impl NewsFilters {
    pub fn order(&self) -> Order {
        if self.mode != SearchMode::Keyword && !self.query.trim().is_empty() {
            Order::Ranked
        } else {
            Order::Newest
        }
    }

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
                .and_then(|m| SearchMode::parse(m))
                .unwrap_or_default(),
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
            params.append_pair("mode", self.mode.as_str());
        }
        params.finish()
    }

    /// The `/news` URL that reproduces these filters; default values are omitted.
    pub fn page_url(&self) -> String {
        let mut params = form_urlencoded::Serializer::new(String::new());
        if !self.query.trim().is_empty() {
            params.append_pair("q", self.query.trim());
        }
        if let Some(id) = self.feed_id {
            params.append_pair("source_id", &id.to_string());
        }
        if self.mode != SearchMode::default() {
            params.append_pair("mode", self.mode.as_str());
        }
        let query = params.finish();
        if query.is_empty() {
            "/news".into()
        } else {
            format!("/news?{query}")
        }
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

pub fn display_order(items: Vec<FeedItem>, order: Order) -> Vec<FeedItem> {
    match order {
        Order::Ranked => items,
        Order::Newest => sort_newest(items),
    }
}

pub fn merge_items(items: &mut Vec<FeedItem>, incoming: impl IntoIterator<Item = FeedItem>) {
    let mut by_id: BTreeMap<_, _> = items.drain(..).map(|i| (i.id, i)).collect();
    by_id.extend(incoming.into_iter().map(|i| (i.id, i)));
    *items = sort_newest(by_id.into_values().collect());
}

/// Adds the next page to `items` and returns whether more pages may follow.
pub fn append_page(items: &mut Vec<FeedItem>, page: Vec<FeedItem>, order: Order) -> bool {
    let has_more = page.len() >= PAGE_SIZE;
    match order {
        Order::Ranked => {
            let seen: std::collections::HashSet<i64> = items.iter().map(|i| i.id).collect();
            items.extend(page.into_iter().filter(|i| !seen.contains(&i.id)));
        }
        Order::Newest => merge_items(items, page),
    }
    has_more
}

/// Display names by feed id; islands get this instead of the full feed list.
pub type FeedNames = BTreeMap<i64, String>;

pub fn feed_names(feeds: &[Feed]) -> FeedNames {
    feeds
        .iter()
        .map(|feed| (feed.id, feed.name().to_owned()))
        .collect()
}

pub fn feed_name(names: &FeedNames, id: i64) -> String {
    names.get(&id).cloned().unwrap_or_default()
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

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: i64) -> FeedItem {
        FeedItem {
            id,
            inserted_at: "2025-01-01T00:00:00Z".into(),
            ..Default::default()
        }
    }

    fn ids(items: &[FeedItem]) -> Vec<i64> {
        items.iter().map(|i| i.id).collect()
    }

    #[test]
    fn publication_is_capped_at_insertion_and_ties_use_id() {
        let mut future = item(1);
        future.published_at = Some("2030-01-01T00:00:00Z".into());
        assert_eq!(effective_date(&future), effective_date(&item(2)));
        let mut older = item(3);
        older.published_at = Some("2024-01-01T00:00:00Z".into());
        assert_eq!(ids(&sort_newest(vec![future, older, item(2)])), [2, 1, 3]);
    }

    #[test]
    fn only_ranked_searches_keep_backend_order() {
        let order = |query: &str| NewsFilters::parse(query).order();
        assert_eq!(order("q=rust"), Order::Ranked);
        assert_eq!(order("q=rust&mode=semantic"), Order::Ranked);
        assert_eq!(order("q=rust&mode=keyword"), Order::Newest);
        assert_eq!(order("q=+&mode=semantic"), Order::Newest);
        assert_eq!(
            ids(&display_order(vec![item(1), item(2)], Order::Ranked)),
            [1, 2]
        );
        assert_eq!(
            ids(&display_order(vec![item(1), item(2)], Order::Newest)),
            [2, 1]
        );
    }

    #[test]
    fn search_modes_round_trip_through_urls_and_json() {
        for mode in SearchMode::ALL {
            assert_eq!(SearchMode::parse(mode.as_str()), Some(mode));
            assert_eq!(
                serde_json::to_string(&mode).unwrap(),
                format!("\"{}\"", mode.as_str())
            );
        }
        assert_eq!(SearchMode::parse("invalid"), None);
        assert_eq!(SearchMode::default(), SearchMode::Hybrid);
    }

    #[test]
    fn filters_keep_legacy_source_alias_and_default_to_hybrid() {
        let filters = NewsFilters::parse("q=+rust+&feed_id=3&mode=invalid");
        assert_eq!(filters.query, "rust");
        assert_eq!(filters.feed_id, Some(3));
        assert_eq!(filters.mode, SearchMode::Hybrid);
        assert_eq!(NewsFilters::parse("source_id=-1&feed_id=3").feed_id, None);
        assert_eq!(NewsFilters::parse("source_id=2&feed_id=3").feed_id, Some(2));
        assert_eq!(NewsFilters::parse("source_id=1.5").feed_id, None);
    }

    #[test]
    fn api_params_omit_mode_without_query_and_use_feed_id() {
        assert_eq!(NewsFilters::default().api_query(0), "limit=50");
        assert_eq!(NewsFilters::default().api_query(100), "limit=50&offset=100");
        let filters = NewsFilters::parse("q=rust+%26+web&source_id=42&mode=semantic");
        assert_eq!(
            filters.api_query(0),
            "limit=50&feed_id=42&q=rust+%26+web&mode=semantic"
        );
        assert_eq!(
            topic_url("rust & web", 42),
            "/news?q=rust+%26+web&source_id=42"
        );
    }

    #[test]
    fn page_urls_round_trip_and_omit_defaults() {
        assert_eq!(NewsFilters::default().page_url(), "/news");
        let filters = NewsFilters::parse("q=rust+%26+web&source_id=42&mode=semantic");
        assert_eq!(
            filters.page_url(),
            "/news?q=rust+%26+web&source_id=42&mode=semantic"
        );
        let url = filters.page_url();
        assert_eq!(NewsFilters::parse(url.trim_start_matches("/news")), filters);
        let hybrid = NewsFilters::parse("q=rust");
        assert_eq!(hybrid.page_url(), "/news?q=rust");
    }

    #[test]
    fn previews_are_plain_text_and_truncate_unicode_safely() {
        let mut value = item(1);
        value.summary = Some("<p>Hello</p>  <b>world</b>".into());
        assert_eq!(preview(&value), "Hello world");
        value.summary = Some("<br>".into());
        value.content = Some("fallback".into());
        assert_eq!(preview(&value), "fallback");
        assert_eq!(truncate("🦀rust", 3), "🦀r…");
        assert_eq!(hostname("https://www.example.com/path"), "example.com");
        assert_eq!(hostname("bad url"), "");
    }

    #[test]
    fn relative_dates_and_poll_intervals() {
        let now = timestamp("2025-01-01T01:00:00Z");
        assert_eq!(time_ago("2025-01-01T00:59:30Z", now), "just now");
        assert_eq!(time_ago("2025-01-01T00:00:00Z", now), "1h ago");
        assert_eq!(poll_interval(30), "30s");
        assert_eq!(poll_interval(90), "1m 30s");
        assert_eq!(poll_interval(3660), "1h 1m");
    }

    #[test]
    fn slim_items_keep_only_the_rendered_preview() {
        let mut value = item(1);
        value.summary = Some(String::new());
        value.content = Some(format!("<p>{}</p>", "word ".repeat(200)));
        let card_preview = truncate(&preview(&value), PREVIEW_CHARS);
        let slim = slim_item(value);
        assert_eq!(slim.content, None);
        assert_eq!(slim.summary.as_deref(), Some(card_preview.as_str()));
        assert_eq!(truncate(&preview(&slim), PREVIEW_CHARS), card_preview);
    }

    #[test]
    fn pages_append_without_duplicates_and_detect_the_end() {
        let mut newest = vec![item(3), item(2)];
        assert!(!append_page(
            &mut newest,
            vec![item(2), item(1)],
            Order::Newest
        ));
        assert_eq!(ids(&newest), [3, 2, 1]);

        // Ranked results keep backend order: the next page goes after, not re-sorted.
        let mut ranked = vec![item(1), item(3)];
        append_page(&mut ranked, vec![item(3), item(5), item(2)], Order::Ranked);
        assert_eq!(ids(&ranked), [1, 3, 5, 2]);

        let full = (1..=PAGE_SIZE as i64).map(item).collect::<Vec<_>>();
        assert!(append_page(&mut vec![], full, Order::Newest));
        assert!(!append_page(&mut vec![], vec![item(1)], Order::Newest));
    }

    #[test]
    fn feed_names_survive_island_serialization() {
        let feeds = [
            Feed {
                id: 1,
                url: "https://a.example/feed".into(),
                title: Some("A".into()),
                ..Default::default()
            },
            Feed {
                id: 2,
                url: "https://b.example/feed".into(),
                ..Default::default()
            },
        ];
        let names = feed_names(&feeds);
        let names: FeedNames =
            serde_json::from_str(&serde_json::to_string(&names).unwrap()).unwrap();
        assert_eq!(feed_name(&names, 1), "A");
        assert_eq!(feed_name(&names, 2), "https://b.example/feed");
        assert_eq!(feed_name(&names, 3), "");
    }
}
