use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use url::form_urlencoded;

use super::PAGE_SIZE;

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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[cfg(not(target_arch = "wasm32"))]
    mod properties {
        use super::*;

        quickcheck::quickcheck! {
            fn prop_page_urls_round_trip_normalized_filters(query: String, feed_id: Option<i64>, mode: u8) -> bool {
                let filters = NewsFilters {
                    query: query.trim().to_owned(),
                    feed_id: feed_id.filter(|id| *id > 0),
                    mode: SearchMode::ALL[usize::from(mode) % SearchMode::ALL.len()],
                };
                NewsFilters::parse(filters.page_url().trim_start_matches("/news")) == filters
            }
        }
    }
}
