use chrono::DateTime;

use super::items::timestamp;
use crate::models::FeedItem;

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

    #[test]
    fn previews_are_plain_text_and_truncate_unicode_safely() {
        let mut value = FeedItem {
            summary: Some("<p>Hello</p>  <b>world</b>".into()),
            ..Default::default()
        };
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

    #[cfg(not(target_arch = "wasm32"))]
    mod properties {
        use super::*;

        quickcheck::quickcheck! {
            fn prop_truncate_respects_the_limit_and_keeps_short_text(value: String, max: u8) -> bool {
                let max = usize::from(max) + 1;
                let truncated = truncate(&value, max);
                truncated.chars().count() <= max
                    && (value.chars().count() > max || truncated == value)
            }

            fn prop_compact_text_is_idempotent_single_spaced_and_tag_free(value: String) -> bool {
                let compact = compact_text(&value);
                compact_text(&compact) == compact
                    && !compact.contains('<')
                    && !compact.contains("  ")
                    && compact.trim() == compact
            }
        }
    }
}
