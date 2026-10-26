use std::collections::{BTreeMap, HashSet};

use chrono::DateTime;

use super::format::{preview, truncate};
use super::{Order, PAGE_SIZE};
use crate::models::{Feed, FeedItem};

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
            let seen: HashSet<i64> = items.iter().map(|i| i.id).collect();
            items.extend(page.into_iter().filter(|i| !seen.contains(&i.id)));
        }
        Order::Newest => merge_items(items, page),
    }
    has_more
}

pub const PREVIEW_CHARS: usize = 240;

/// Drops the full article body so list payloads only carry what a card renders.
pub fn slim_item(mut item: FeedItem) -> FeedItem {
    let text = truncate(&preview(&item), PREVIEW_CHARS);
    item.summary = (!text.is_empty()).then_some(text);
    item.content = None;
    item
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::logic::NewsFilters;

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

    #[cfg(not(target_arch = "wasm32"))]
    mod properties {
        use super::*;

        /// Items with small ids (so merges overlap) and dates a few days apart.
        fn dated_items(specs: &[(u8, u8, Option<u8>)]) -> Vec<FeedItem> {
            let date = |day: u8| format!("2025-01-{:02}T00:00:00Z", day % 28 + 1);
            specs
                .iter()
                .map(|(id, inserted, published)| FeedItem {
                    id: i64::from(id % 32),
                    inserted_at: date(*inserted),
                    published_at: published.map(date),
                    ..Default::default()
                })
                .collect()
        }

        fn is_newest_first(items: &[FeedItem]) -> bool {
            items
                .windows(2)
                .all(|w| (effective_date(&w[0]), w[0].id) > (effective_date(&w[1]), w[1].id))
        }

        quickcheck::quickcheck! {
            fn prop_merged_items_are_unique_sorted_and_idempotent(
                existing: Vec<(u8, u8, Option<u8>)>,
                incoming: Vec<(u8, u8, Option<u8>)>
            ) -> bool {
                let mut merged = Vec::new();
                merge_items(&mut merged, dated_items(&existing));
                merge_items(&mut merged, dated_items(&incoming));
                let mut again = merged.clone();
                merge_items(&mut again, dated_items(&incoming));
                let expected_ids: std::collections::BTreeSet<i64> = dated_items(&existing)
                    .iter()
                    .chain(&dated_items(&incoming))
                    .map(|item| item.id)
                    .collect();
                is_newest_first(&merged)
                    && ids(&merged).into_iter().collect::<std::collections::BTreeSet<_>>() == expected_ids
                    && again == merged
            }
        }
    }
}
