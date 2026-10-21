use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FeedItem {
    pub id: i64,
    pub feed_id: i64,
    pub external_id: String,
    pub title: String,
    pub url: String,
    pub inserted_at: String,
    pub summary: Option<String>,
    pub content: Option<String>,
    pub author: Option<String>,
    pub published_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ItemDetail {
    pub summary: String,
    pub content: String,
    pub author: String,
    pub published_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DailySummary {
    pub id: i64,
    pub summary: String,
    pub feed_ids: Vec<i64>,
    pub feed_item_ids: Vec<i64>,
    pub generated_at: String,
    pub model: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Feed {
    pub id: i64,
    pub url: String,
    pub title: Option<String>,
    pub site_url: Option<String>,
    pub poll_interval_seconds: i64,
    pub is_enabled: bool,
    pub last_checked_at: Option<String>,
    pub last_success_at: Option<String>,
    pub last_inserted_at: Option<String>,
    pub failure_count: i64,
}

impl Feed {
    pub fn name(&self) -> &str {
        self.title
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .or_else(|| {
                self.site_url
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            })
            .unwrap_or(&self.url)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub word: String,
    #[serde(rename = "total_occurences", alias = "total_occurrences")]
    pub total_occurrences: i64,
    pub document_frequency: i64,
    pub tf_idf: f64,
    pub items: Vec<TopicSource>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TopicSource {
    pub feed_src_id: i64,
    #[serde(rename = "occurences", alias = "occurrences")]
    pub occurrences: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReplayDone {
    pub replayed: usize,
    pub limited: bool,
    pub last_event_id: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
