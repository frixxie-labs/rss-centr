use chrono::{DateTime, Utc};
use feed_rs::model::{Entry, Feed, Text};
use rss_centr_core::feed_update_queue::FeedUpdateItemInput;

pub(crate) fn feed_to_items(feed: &Feed) -> Vec<FeedUpdateItemInput> {
    feed.entries.iter().map(entry_to_item).collect()
}

pub(crate) fn feed_title_and_site_url(feed: &Feed) -> (Option<String>, Option<String>) {
    let title = feed.title.as_ref().map(|t| text_value(t).to_string());
    let site_url = feed.links.first().map(|link| link.href.clone());
    (title, site_url)
}

fn entry_to_item(entry: &Entry) -> FeedUpdateItemInput {
    let now = Utc::now();
    let (summary, content) = entry_summary_and_content(entry);

    FeedUpdateItemInput {
        external_id: entry_external_id(entry),
        title: entry
            .title
            .as_ref()
            .map(text_value)
            .unwrap_or("(no title)")
            .to_string(),
        url: entry
            .links
            .first()
            .map(|link| link.href.clone())
            .unwrap_or_default(),
        summary: Some(summary),
        content: Some(content),
        author: Some(
            entry
                .authors
                .first()
                .map(|author| author.name.clone())
                .unwrap_or_default(),
        ),
        published_at: Some(entry_published_at(entry).unwrap_or(now)),
    }
}

fn text_value(t: &Text) -> &str {
    t.content.as_str()
}

fn entry_external_id(entry: &Entry) -> String {
    if !entry.id.is_empty() {
        return entry.id.clone();
    }

    if let Some(link) = entry.links.first() {
        return link.href.clone();
    }

    let title = entry
        .title
        .as_ref()
        .map(text_value)
        .unwrap_or("")
        .to_string();
    let published = entry_published_at(entry)
        .map(|d| d.to_rfc3339())
        .unwrap_or_default();

    format!("fallback:{title}:{published}")
}

fn entry_summary_and_content(entry: &Entry) -> (String, String) {
    let summary = entry
        .summary
        .as_ref()
        .map(text_value)
        .unwrap_or("")
        .to_string();

    let content = entry
        .content
        .as_ref()
        .and_then(|content| content.body.as_deref())
        .unwrap_or("")
        .to_string();

    (summary, content)
}

fn entry_published_at(entry: &Entry) -> Option<DateTime<Utc>> {
    entry
        .published
        .or(entry.updated)
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use feed_rs::model::Link;
    use feed_rs::parser;
    use quickcheck::TestResult;

    fn link(href: String) -> Link {
        Link {
            href,
            rel: None,
            media_type: None,
            href_lang: None,
            title: None,
            length: None,
        }
    }

    fn datetime(seconds: i32) -> DateTime<Utc> {
        DateTime::from_timestamp(i64::from(seconds), 0).unwrap()
    }

    // These mirror the equivalent properties in `backend/src/feed/ingest.rs`
    // for the same fallback logic, duplicated here because the worker has no
    // compile-time dependency on the backend crate.
    quickcheck::quickcheck! {
        fn prop_entry_external_id_prefers_non_empty_id(id: String, href: String) -> TestResult {
            if id.is_empty() {
                return TestResult::discard();
            }

            let entry = Entry {
                id: id.clone(),
                links: vec![link(href)],
                ..Default::default()
            };

            TestResult::from_bool(entry_external_id(&entry) == id)
        }

        fn prop_entry_external_id_uses_first_link_when_id_is_empty(href: String, other_href: String) -> bool {
            let entry = Entry {
                id: String::new(),
                links: vec![link(href.clone()), link(other_href)],
                ..Default::default()
            };

            entry_external_id(&entry) == href
        }

        fn prop_entry_published_at_prefers_published(published_seconds: i32, updated_seconds: i32) -> bool {
            let published = datetime(published_seconds);
            let updated = datetime(updated_seconds);
            let entry = Entry {
                published: Some(published),
                updated: Some(updated),
                ..Default::default()
            };

            entry_published_at(&entry) == Some(published)
        }

        fn prop_entry_published_at_uses_updated_without_published(updated_seconds: i32) -> bool {
            let updated = datetime(updated_seconds);
            let entry = Entry {
                updated: Some(updated),
                ..Default::default()
            };

            entry_published_at(&entry) == Some(updated)
        }
    }

    // -----------------------------------------------------------------------
    // `text_value`, `entry_summary_and_content`, `feed_title_and_site_url` and
    // `entry_to_item` all read data out of `feed_rs::model::{Feed, Text}`
    // values, but those types have no public constructor or `Default` impl
    // outside the `feed_rs` crate itself (by design, since only its own
    // parser is meant to build them). So rather than fighting that, these
    // tests drive the real parser with a small fixture document and assert
    // on what we hand off to the backend -- which also happens to exercise
    // the exact code path (`parser::parse` output feeding `entry_to_item`)
    // that `fetch_feed` uses in production.
    // -----------------------------------------------------------------------

    const SAMPLE_ATOM_FEED: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Feed</title>
  <link href="https://example.com" rel="alternate"/>
  <id>urn:uuid:example-feed</id>
  <entry>
    <title>First Post</title>
    <link href="https://example.com/1" rel="alternate"/>
    <id>https://example.com/1</id>
    <summary>Summary of first post</summary>
    <content type="html">Full content</content>
    <author><name>Jane Doe</name></author>
    <published>2026-07-01T10:00:00Z</published>
  </entry>
  <entry>
    <link href="https://example.com/2" rel="alternate"/>
    <id>https://example.com/2</id>
  </entry>
</feed>"#;

    fn parse_sample() -> Feed {
        parser::parse(SAMPLE_ATOM_FEED.as_bytes()).expect("sample feed should parse")
    }

    #[test]
    fn test_feed_title_and_site_url_reads_feed_metadata() {
        let feed = parse_sample();
        let (title, site_url) = feed_title_and_site_url(&feed);

        assert_eq!(title.as_deref(), Some("Example Feed"));
        // feed-rs normalizes a bare-domain href by appending a trailing
        // slash (via the `url` crate), even though the fixture below
        // writes `href="https://example.com"` with none.
        assert_eq!(site_url.as_deref(), Some("https://example.com/"));
    }

    #[test]
    fn test_text_value_returns_underlying_content() {
        let feed = parse_sample();
        let title = feed.title.as_ref().expect("feed has a title");

        assert_eq!(text_value(title), "Example Feed");
    }

    #[test]
    fn test_entry_to_item_maps_fully_populated_entry() {
        let feed = parse_sample();
        let entry = &feed.entries[0];

        let item = entry_to_item(entry);

        assert_eq!(item.external_id, "https://example.com/1");
        assert_eq!(item.title, "First Post");
        assert_eq!(item.url, "https://example.com/1");
        assert_eq!(item.summary.as_deref(), Some("Summary of first post"));
        assert_eq!(item.content.as_deref(), Some("Full content"));
        assert_eq!(item.author.as_deref(), Some("Jane Doe"));
        assert_eq!(
            item.published_at,
            Some(
                DateTime::parse_from_rfc3339("2026-07-01T10:00:00Z")
                    .unwrap()
                    .with_timezone(&Utc)
            )
        );
    }

    #[test]
    fn test_entry_to_item_falls_back_for_sparse_entry() {
        let feed = parse_sample();
        let entry = &feed.entries[1];

        let item = entry_to_item(entry);

        assert_eq!(item.external_id, "https://example.com/2");
        // No <title>: falls back to a placeholder instead of panicking.
        assert_eq!(item.title, "(no title)");
        // No <summary>/<content>/<author>: the worker always sends `Some("")`
        // rather than `None` for missing detail fields. This matters because
        // the backend's `item_has_detail` check only skips inserting a detail
        // row when every one of these is `None` -- which this producer never
        // sends, so that skip path is currently unreachable in production.
        assert_eq!(item.summary.as_deref(), Some(""));
        assert_eq!(item.content.as_deref(), Some(""));
        assert_eq!(item.author.as_deref(), Some(""));
        // No <published>: falls back to "now" rather than `None`.
        assert!(item.published_at.is_some());
    }
}
