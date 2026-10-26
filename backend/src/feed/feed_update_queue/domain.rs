use std::{
    collections::{HashMap, HashSet},
    error::Error,
    fmt,
};

use chrono::{DateTime, Utc};

use rss_centr_core::feed_update_queue::FeedUpdateItemInput;

#[derive(Debug)]
pub enum FeedQueueError {
    LeaseConflict { feed_id: i64 },
    FeedPaused { feed_id: i64 },
    FeedNotFound { feed_id: i64 },
    Storage(anyhow::Error),
}

impl fmt::Display for FeedQueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LeaseConflict { feed_id } => {
                write!(f, "feed update lease conflict for feed_id={feed_id}")
            }
            Self::FeedPaused { feed_id } => write!(f, "feed is paused for feed_id={feed_id}"),
            Self::FeedNotFound { feed_id } => write!(f, "no feed found with id={feed_id}"),
            Self::Storage(error) => error.fmt(f),
        }
    }
}

impl Error for FeedQueueError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error.as_ref()),
            _ => None,
        }
    }
}

impl From<anyhow::Error> for FeedQueueError {
    fn from(error: anyhow::Error) -> Self {
        Self::Storage(error)
    }
}

/// The items to insert as one column per field, keeping the first occurrence
/// of a repeated `external_id`.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ItemColumns {
    pub(super) external_ids: Vec<String>,
    pub(super) titles: Vec<String>,
    pub(super) urls: Vec<String>,
}

impl ItemColumns {
    pub(super) fn new(items: &[FeedUpdateItemInput]) -> Self {
        let mut seen = HashSet::new();
        items
            .iter()
            .filter(|item| seen.insert(item.external_id.as_str()))
            .fold(Self::default(), |mut columns, item| {
                columns.external_ids.push(item.external_id.clone());
                columns.titles.push(item.title.clone());
                columns.urls.push(item.url.clone());
                columns
            })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.external_ids.is_empty()
    }
}

/// Details of the newly inserted items that carry any, one column per field.
/// Missing text fields become empty and a missing publish date becomes
/// `completed_at`.
#[derive(Debug, Default, PartialEq)]
pub(super) struct DetailColumns {
    pub(super) feed_item_ids: Vec<i64>,
    pub(super) summaries: Vec<String>,
    pub(super) contents: Vec<String>,
    pub(super) authors: Vec<String>,
    pub(super) published_at: Vec<DateTime<Utc>>,
}

impl DetailColumns {
    /// `inserted` pairs each new item id with its `external_id`.
    pub(super) fn new(
        items: &[FeedUpdateItemInput],
        inserted: &[(i64, String)],
        completed_at: DateTime<Utc>,
    ) -> Self {
        let mut by_external_id = HashMap::new();
        for item in items {
            by_external_id
                .entry(item.external_id.as_str())
                .or_insert(item);
        }
        inserted
            .iter()
            .filter_map(|(id, external_id)| {
                by_external_id
                    .get(external_id.as_str())
                    .filter(|item| has_detail(item))
                    .map(|item| (*id, *item))
            })
            .fold(Self::default(), |mut columns, (id, item)| {
                columns.feed_item_ids.push(id);
                columns
                    .summaries
                    .push(item.summary.clone().unwrap_or_default());
                columns
                    .contents
                    .push(item.content.clone().unwrap_or_default());
                columns
                    .authors
                    .push(item.author.clone().unwrap_or_default());
                columns
                    .published_at
                    .push(item.published_at.unwrap_or(completed_at));
                columns
            })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.feed_item_ids.is_empty()
    }
}

fn has_detail(item: &FeedUpdateItemInput) -> bool {
    item.summary.is_some()
        || item.content.is_some()
        || item.author.is_some()
        || item.published_at.is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Items from `(external_id key, has summary)` pairs; small keys force repeats.
    fn items(keys: &[(u8, bool)]) -> Vec<FeedUpdateItemInput> {
        keys.iter()
            .enumerate()
            .map(|(position, (key, summary))| FeedUpdateItemInput {
                external_id: (key % 8).to_string(),
                title: format!("title {position}"),
                url: format!("https://example.com/{position}"),
                summary: summary.then(|| format!("summary {position}")),
                content: None,
                author: None,
                published_at: None,
            })
            .collect()
    }

    quickcheck::quickcheck! {
        fn prop_item_columns_keep_first_occurrence_in_order(keys: Vec<(u8, bool)>) -> bool {
            let items = items(&keys);
            let columns = ItemColumns::new(&items);
            let mut seen = HashSet::new();
            let first: Vec<&FeedUpdateItemInput> = items
                .iter()
                .filter(|item| seen.insert(item.external_id.clone()))
                .collect();
            columns.external_ids == first.iter().map(|i| i.external_id.clone()).collect::<Vec<_>>()
                && columns.titles == first.iter().map(|i| i.title.clone()).collect::<Vec<_>>()
                && columns.urls == first.iter().map(|i| i.url.clone()).collect::<Vec<_>>()
        }

        fn prop_detail_columns_cover_inserted_items_with_details(keys: Vec<(u8, bool)>, skip: Vec<bool>) -> bool {
            let items = items(&keys);
            let columns = ItemColumns::new(&items);
            let inserted: Vec<(i64, String)> = columns
                .external_ids
                .iter()
                .enumerate()
                .filter(|(index, _)| !skip.get(*index).copied().unwrap_or(false))
                .map(|(index, external_id)| (index as i64 + 100, external_id.clone()))
                .collect();
            let details = DetailColumns::new(&items, &inserted, DateTime::UNIX_EPOCH);
            let first = |external_id: &str| items.iter().find(|i| i.external_id == external_id).unwrap();
            let expected: Vec<(i64, String)> = inserted
                .iter()
                .filter(|(_, external_id)| first(external_id).summary.is_some())
                .map(|(id, external_id)| (*id, first(external_id).summary.clone().unwrap()))
                .collect();
            let n = details.feed_item_ids.len();
            details.feed_item_ids.iter().copied().zip(details.summaries.iter().cloned()).collect::<Vec<_>>() == expected
                && [details.contents.len(), details.authors.len(), details.published_at.len()] == [n; 3]
                && details.published_at.iter().all(|date| *date == DateTime::UNIX_EPOCH)
        }
    }
}
