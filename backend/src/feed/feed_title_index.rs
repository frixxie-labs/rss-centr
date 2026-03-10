use std::collections::HashMap;

use nom::{
    bytes::complete::take_while1,
    character::complete::{multispace0, multispace1},
    multi::separated_list0,
    sequence::preceded,
    IResult, Parser,
};
use unicode_normalization::UnicodeNormalization;

use crate::feed::feed_item::FeedItem;

fn normalize_word(word: &str) -> String {
    word.nfc()
        .filter(|c| c.is_alphanumeric())
        .collect::<String>()
        .to_lowercase()
}

fn parse_word(input: &str) -> IResult<&str, &str> {
    take_while1(|c: char| !c.is_whitespace()).parse(input)
}

fn parse_words(input: &str) -> IResult<&str, Vec<&str>> {
    preceded(multispace0, separated_list0(multispace1, parse_word)).parse(input)
}

#[derive(Debug, Ord, PartialEq, PartialOrd, Eq, serde::Serialize)]
pub struct FeedTitleIndexItem {
    pub feed_src_id: i64,
    pub occurences: u64,
}

impl FeedTitleIndexItem {
    pub fn new(feed_src_id: i64, occurences: u64) -> Self {
        Self {
            feed_src_id,
            occurences,
        }
    }
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct FeedTitleIndexEntry {
    pub word: String,
    pub total_occurences: u64,
    pub items: Vec<FeedTitleIndexItem>,
}

#[derive(Debug)]
pub struct FeedTitleIndex {
    pub items: HashMap<String, Vec<FeedTitleIndexItem>>,
    pub total_items: i32,
}

impl FeedTitleIndex {
    pub fn new() -> Self {
        Self {
            items: HashMap::new(),
            total_items: 0,
        }
    }

    pub fn add_item(&mut self, title: String, feed_src_id: i64) {
        let words = parse_words(&title)
            .map(|(_, words)| words)
            .unwrap_or_default();
        for word in words {
            let key = normalize_word(word);
            if key.is_empty() {
                continue;
            }
            let entry = self.items.entry(key).or_default();
            if let Some(item) = entry
                .iter_mut()
                .find(|item| item.feed_src_id == feed_src_id)
            {
                item.occurences += 1;
            } else {
                entry.push(FeedTitleIndexItem::new(feed_src_id, 1));
            }
        }
        self.total_items += 1;
    }

    pub fn get_items(&self, word: &str) -> Option<&Vec<FeedTitleIndexItem>> {
        let key = normalize_word(word);
        self.items.get(&key)
    }

    pub fn export_index(self) -> Vec<FeedTitleIndexEntry> {
        let mut entries: Vec<FeedTitleIndexEntry> = self
            .items
            .into_iter()
            .map(|(word, mut items)| {
                let total_occurences: u64 = items.iter().map(|i| i.occurences).sum();
                items.sort_by(|a, b| b.occurences.cmp(&a.occurences));
                FeedTitleIndexEntry {
                    word,
                    total_occurences,
                    items,
                }
            })
            .collect();
        entries.sort_by(|a, b| b.total_occurences.cmp(&a.total_occurences));
        entries
    }

    pub fn get_total_items(&self) -> i32 {
        self.total_items
    }
}

impl From<Vec<FeedItem>> for FeedTitleIndex {
    fn from(items: Vec<FeedItem>) -> Self {
        let mut index = FeedTitleIndex::new();
        for item in items {
            index.add_item(item.title, item.feed_id.clone());
        }
        index
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::{
        feed_item::{insert_feed_item, read_all_feed_items},
        feed_subscription::upsert_feed_by_url,
    };

    #[sqlx::test]
    pub async fn test_feed_title_index(pool: sqlx::SqlitePool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();
        let items = read_all_feed_items(&pool).await.unwrap();
        let index = FeedTitleIndex::from(items);

        assert_eq!(index.get_total_items(), 1);
        let title_items = index.get_items("Title").unwrap();
        assert_eq!(title_items.len(), 1);
        assert_eq!(title_items[0].feed_src_id, feed.id);
    }

    #[sqlx::test]
    pub async fn test_feed_title_index_multiple_items(pool: sqlx::SqlitePool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed.id, "ext-1", "Title One", "https://example.com")
            .await
            .unwrap();
        insert_feed_item(&pool, feed.id, "ext-2", "Title Two", "https://example.com")
            .await
            .unwrap();
        let items = read_all_feed_items(&pool).await.unwrap();
        let index = FeedTitleIndex::from(items);

        assert_eq!(index.get_total_items(), 2);
        let title_one_items = index.get_items("One").unwrap();
        assert_eq!(title_one_items.len(), 1);
        assert_eq!(title_one_items[0].feed_src_id, feed.id);

        let title_two_items = index.get_items("Two").unwrap();
        assert_eq!(title_two_items.len(), 1);
        assert_eq!(title_two_items[0].feed_src_id, feed.id);
    }

    #[sqlx::test]
    pub async fn test_feed_title_index_multiple_feeds(pool: sqlx::SqlitePool) {
        let feed1 = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();
        let feed2 = upsert_feed_by_url(&pool, "https://example.com/feed2.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed1.id, "ext-1", "Title One", "https://example.com")
            .await
            .unwrap();
        insert_feed_item(&pool, feed2.id, "ext-2", "Title One", "https://example.com")
            .await
            .unwrap();
        let items = read_all_feed_items(&pool).await.unwrap();
        let index = FeedTitleIndex::from(items);

        assert_eq!(index.get_total_items(), 2);
        let title_one_items = index.get_items("One").unwrap();
        assert_eq!(title_one_items.len(), 2);
        assert!(
            title_one_items
                .iter()
                .any(|item| item.feed_src_id == feed1.id)
        );
        assert!(
            title_one_items
                .iter()
                .any(|item| item.feed_src_id == feed2.id)
        );
    }

    #[sqlx::test]
    pub async fn test_feed_title_index_no_items(_pool: sqlx::SqlitePool) {
        let index = FeedTitleIndex::new();
        assert_eq!(index.get_total_items(), 0);
        assert!(index.get_items("Nonexistent").is_none());
    }

    #[sqlx::test]
    pub async fn test_export_index(pool: sqlx::SqlitePool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed.id, "ext-1", "Title One", "https://example.com")
            .await
            .unwrap();
        let items = read_all_feed_items(&pool).await.unwrap();
        let index = FeedTitleIndex::from(items);
        let exported = index.export_index();

        assert_eq!(exported.len(), 2); // "title" and "one"
        assert!(exported.iter().any(|e| e.word == "title"));
        assert!(exported.iter().any(|e| e.word == "one"));
        for entry in &exported {
            assert_eq!(entry.total_occurences, 1);
            assert_eq!(entry.items.len(), 1);
        }
    }

    #[sqlx::test]
    pub async fn test_export_index_sorted_by_total_occurences(pool: sqlx::SqlitePool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        // "title" appears 3 times, "one" 1 time, "two" 1 time
        insert_feed_item(&pool, feed.id, "ext-1", "Title One", "https://example.com")
            .await
            .unwrap();
        insert_feed_item(&pool, feed.id, "ext-2", "Title Two", "https://example.com")
            .await
            .unwrap();
        insert_feed_item(&pool, feed.id, "ext-3", "Title", "https://example.com")
            .await
            .unwrap();
        let items = read_all_feed_items(&pool).await.unwrap();
        let index = FeedTitleIndex::from(items);
        let exported = index.export_index();

        assert_eq!(exported.len(), 3);
        // First entry should be "title" with total 3
        assert_eq!(exported[0].word, "title");
        assert_eq!(exported[0].total_occurences, 3);
        // Remaining entries each have total 1
        assert_eq!(exported[1].total_occurences, 1);
        assert_eq!(exported[2].total_occurences, 1);
    }
}
