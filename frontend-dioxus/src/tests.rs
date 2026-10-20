use crate::{logic::*, models::*};

fn item(id: i64) -> FeedItem {
    FeedItem {
        id,
        inserted_at: "2025-01-01T00:00:00Z".into(),
        ..Default::default()
    }
}

#[test]
fn publication_is_capped_at_insertion_and_ties_use_id() {
    let mut future = item(1);
    future.published_at = Some("2030-01-01T00:00:00Z".into());
    assert_eq!(effective_date(&future), effective_date(&item(2)));
    let mut older = item(3);
    older.published_at = Some("2024-01-01T00:00:00Z".into());
    assert_eq!(
        sort_newest(vec![future, older, item(2)])
            .iter()
            .map(|i| i.id)
            .collect::<Vec<_>>(),
        [2, 1, 3]
    );
}

#[test]
fn ranked_search_preserves_backend_order() {
    assert_eq!(
        display_order(vec![item(1), item(2)], "hybrid", "rust")[0].id,
        1
    );
    assert_eq!(
        display_order(vec![item(1), item(2)], "semantic", "rust")[0].id,
        1
    );
    assert_eq!(
        display_order(vec![item(1), item(2)], "keyword", "rust")[0].id,
        2
    );
    assert_eq!(
        display_order(vec![item(1), item(2)], "hybrid", " ")[0].id,
        2
    );
}

#[test]
fn filters_keep_legacy_source_alias_and_default_to_hybrid() {
    let filters = NewsFilters::parse("q=+rust+&feed_id=3&mode=invalid");
    assert_eq!(filters.query, "rust");
    assert_eq!(filters.feed_id, Some(3));
    assert_eq!(filters.mode, "hybrid");
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
fn replay_batches_deduplicate_and_replace_details() {
    let mut state = TimelineState::new((1..=501).map(item).collect());
    assert_eq!(state.items.len(), 501);
    assert_eq!(state.items[0].id, 501);
    let mut updated = item(501);
    updated.title = "Updated".into();
    state.receive(updated, "501".into());
    state.receive(item(502), "502".into());
    assert_eq!(state.items[0].id, 501); // replay is buffered
    assert!(state.finish_replay(ReplayDone {
        replayed: 2,
        limited: true,
        last_event_id: Some(502)
    }));
    assert_eq!(state.items.len(), 502);
    assert_eq!(state.items[0].id, 502);
    assert_eq!(state.items[1].title, "Updated");
    assert_eq!(state.cursor.as_deref(), Some("502"));
    assert!(state.limited);
    state.replaying = true;
    state.receive(item(503), "503".into());
    assert!(state.finish_replay(ReplayDone::default()));
    assert_eq!(state.cursor.as_deref(), Some("503"));
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
fn topic_wire_spelling_and_missing_item_details_deserialize() {
    let topic: Topic = serde_json::from_str(r#"{"word":"rust","total_occurences":2,"document_frequency":1,"tf_idf":0.5,"items":[{"feed_src_id":1,"occurences":2}]}"#).unwrap();
    assert_eq!(topic.total_occurrences, 2);
    assert_eq!(topic.items[0].occurrences, 2);
    let event: FeedItem = serde_json::from_str(r#"{"id":1,"feed_id":2,"external_id":"a","title":"x","url":"https://example.com","inserted_at":"2025-01-01T00:00:00Z"}"#).unwrap();
    assert!(event.summary.is_none());
    assert!(serde_json::from_str::<ReplayDone>("not json").is_err());
    assert_eq!(
        serde_json::from_str::<ReplayDone>("{}").unwrap(),
        ReplayDone::default()
    );
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
        "hybrid",
        ""
    ));
    assert_eq!(newest.iter().map(|i| i.id).collect::<Vec<_>>(), [3, 2, 1]);

    // Ranked results keep backend order: the next page goes after, not re-sorted.
    let mut ranked = vec![item(1), item(3)];
    append_page(
        &mut ranked,
        vec![item(3), item(5), item(2)],
        "semantic",
        "rust",
    );
    assert_eq!(
        ranked.iter().map(|i| i.id).collect::<Vec<_>>(),
        [1, 3, 5, 2]
    );

    let full = (1..=PAGE_SIZE as i64).map(item).collect::<Vec<_>>();
    assert!(append_page(&mut vec![], full, "keyword", ""));
    assert!(!append_page(&mut vec![], vec![item(1)], "keyword", ""));
}
