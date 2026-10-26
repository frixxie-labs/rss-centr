use std::{error::Error, fmt};

use chrono::{DateTime, Utc};
use rss_centr_core::semantic::{hybrid_rank, item_date};
use serde::Deserialize;
use tracing::warn;
use utoipa::ToSchema;

use crate::feed::feed_item::read_feed_items_with_detail_by_ids;
use crate::feed::feed_item::{FeedItemWithDetail, read_latest_feed_items_with_detail};

use super::{SearchCounts, SearchStage, SearchState, SearchTimings};

/// Default page size for ranked (semantic/hybrid) searches.
const DEFAULT_SEARCH_LIMIT: usize = 50;
/// Maximum number of keyword matches considered in a hybrid search.
const KEYWORD_CANDIDATES: usize = 500;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum SearchMode {
    #[default]
    Keyword,
    Semantic,
    Hybrid,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LatestItemsQuery {
    pub(crate) limit: Option<u32>,
    pub(crate) offset: Option<u32>,
    pub(crate) feed_id: Option<i64>,
    pub(crate) q: Option<String>,
    #[serde(default)]
    pub(crate) mode: SearchMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RankedMode {
    Semantic,
    Hybrid,
}

impl RankedMode {
    fn name(self) -> &'static str {
        match self {
            Self::Semantic => "semantic",
            Self::Hybrid => "hybrid",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SearchPlan<'a> {
    Latest,
    Keyword(&'a str),
    Ranked { query: &'a str, mode: RankedMode },
}

impl LatestItemsQuery {
    fn plan(&self) -> SearchPlan<'_> {
        let query = self.q.as_deref().map(str::trim).filter(|q| !q.is_empty());
        match (query, self.mode) {
            (None, _) => SearchPlan::Latest,
            (Some(query), SearchMode::Keyword) => SearchPlan::Keyword(query),
            (Some(query), SearchMode::Semantic) => SearchPlan::Ranked {
                query,
                mode: RankedMode::Semantic,
            },
            (Some(query), SearchMode::Hybrid) => SearchPlan::Ranked {
                query,
                mode: RankedMode::Hybrid,
            },
        }
    }
}

#[derive(Debug)]
pub(crate) enum SearchError {
    Storage(anyhow::Error),
    EmbeddingUnavailable,
}

impl fmt::Display for SearchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => error.fmt(f),
            Self::EmbeddingUnavailable => f.write_str("Semantic search is unavailable"),
        }
    }
}

impl Error for SearchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error.as_ref()),
            Self::EmbeddingUnavailable => None,
        }
    }
}

impl From<anyhow::Error> for SearchError {
    fn from(error: anyhow::Error) -> Self {
        Self::Storage(error)
    }
}

/// Execute a search using a fixed ranking time for the entire request.
pub(crate) async fn search_items(
    state: &SearchState,
    query: &LatestItemsQuery,
    now: DateTime<Utc>,
) -> Result<Vec<FeedItemWithDetail>, SearchError> {
    match query.plan() {
        SearchPlan::Latest => chronological_items(state, query, None).await,
        SearchPlan::Keyword(text) => chronological_items(state, query, Some(text)).await,
        SearchPlan::Ranked { query: text, mode } => {
            ranked_items(state, query, text, mode, now).await
        }
    }
}

async fn chronological_items(
    state: &SearchState,
    query: &LatestItemsQuery,
    text: Option<&str>,
) -> Result<Vec<FeedItemWithDetail>, SearchError> {
    Ok(read_latest_feed_items_with_detail(
        &state.pool,
        query.limit.map(i64::from),
        query.feed_id,
        text,
        query.offset.map(i64::from),
    )
    .await?)
}

async fn ranked_items(
    state: &SearchState,
    query: &LatestItemsQuery,
    text: &str,
    mode: RankedMode,
    now: DateTime<Utc>,
) -> Result<Vec<FeedItemWithDetail>, SearchError> {
    // Ranking is not incremental: rank up to the end of the page, then skip the offset.
    let offset = query.offset.unwrap_or(0) as usize;
    let limit = query.limit.map_or(DEFAULT_SEARCH_LIMIT, |l| l as usize) + offset;
    let mut timings = SearchTimings::new(mode.name());
    let mut counts = SearchCounts::default();
    let query_vector = match timings
        .time(SearchStage::Embed, state.embedder.embed_query(text))
        .await
    {
        Ok(vector) => Some(vector),
        Err(e) => {
            warn!("failed to embed search query: {e:#}");
            None
        }
    };
    if mode == RankedMode::Semantic && query_vector.is_none() {
        return Err(SearchError::EmbeddingUnavailable);
    }

    let keyword_items = if mode == RankedMode::Hybrid {
        timings
            .time(
                SearchStage::Keyword,
                read_latest_feed_items_with_detail(
                    &state.pool,
                    Some(KEYWORD_CANDIDATES.max(limit) as i64),
                    query.feed_id,
                    Some(text),
                    None,
                ),
            )
            .await?
    } else {
        Vec::new()
    };
    let keyword_ids: Vec<i64> = keyword_items.iter().map(|item| item.id).collect();
    counts.keyword_matches = keyword_ids.len();

    let ids = match query_vector {
        Some(vector) => {
            let index = &state.embeddings;
            timings
                .time(SearchStage::Refresh, index.refresh(&state.pool))
                .await?;
            let similarities = timings
                .time(
                    SearchStage::Scan,
                    index.similarities(&vector, query.feed_id),
                )
                .await;
            counts.indexed = index.item_count().await;
            counts.scored = similarities.len();
            timings
                .time(SearchStage::Rank, async {
                    let mut dates = index.dates(similarities.iter().map(|(id, _)| *id)).await;
                    dates.extend(
                        keyword_items
                            .iter()
                            .map(|item| (item.id, item_date(item.published_at, item.inserted_at))),
                    );
                    hybrid_rank(
                        &similarities,
                        &keyword_ids,
                        state.min_similarity,
                        limit,
                        |id| {
                            dates
                                .get(&id)
                                .map_or(1.0, |date| state.recency.factor(*date, now))
                        },
                    )
                })
                .await
        }
        None => keyword_ids.into_iter().take(limit).collect(),
    };

    let ids: Vec<i64> = ids.into_iter().skip(offset).collect();
    let items = timings
        .time(
            SearchStage::Load,
            read_feed_items_with_detail_by_ids(&state.pool, &ids),
        )
        .await?;
    counts.results = items.len();
    timings.finish(counts);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Duration;
    use rss_centr_core::semantic::{ItemEmbedding, RecencyBoost};
    use sqlx::PgPool;

    use super::*;
    use crate::feed::feed_item::insert_feed_item;
    use crate::feed::feed_item_detail::insert_feed_item_detail;
    use crate::feed::feed_subscription::upsert_feed_by_url;
    use crate::search::embedding_store::store_item_embeddings;
    use crate::search::{EmbedFuture, EmbeddingIndex, QueryEmbedder};

    struct FixedEmbedder;

    impl QueryEmbedder for FixedEmbedder {
        fn embed_query<'a>(&'a self, query: &'a str) -> EmbedFuture<'a> {
            Box::pin(async move {
                assert_eq!(query, "news");
                Ok(vec![1.0, 0.0])
            })
        }
    }

    #[test]
    fn missing_or_blank_queries_always_use_the_latest_items_plan() {
        for mode in [
            SearchMode::Keyword,
            SearchMode::Semantic,
            SearchMode::Hybrid,
        ] {
            for q in [None, Some("".into()), Some(" \t\n ".into())] {
                let query = LatestItemsQuery {
                    limit: None,
                    offset: None,
                    feed_id: None,
                    q,
                    mode,
                };
                assert_eq!(query.plan(), SearchPlan::Latest);
            }
        }
    }

    #[test]
    fn nonempty_queries_are_trimmed_and_keep_their_search_mode() {
        for (mode, expected) in [
            (SearchMode::Keyword, SearchPlan::Keyword("rust")),
            (
                SearchMode::Semantic,
                SearchPlan::Ranked {
                    query: "rust",
                    mode: RankedMode::Semantic,
                },
            ),
            (
                SearchMode::Hybrid,
                SearchPlan::Ranked {
                    query: "rust",
                    mode: RankedMode::Hybrid,
                },
            ),
        ] {
            let query = LatestItemsQuery {
                limit: Some(10),
                offset: Some(20),
                feed_id: Some(1),
                q: Some("  rust\t".into()),
                mode,
            };
            assert_eq!(query.plan(), expected);
        }
    }

    #[sqlx::test]
    async fn ranked_recency_uses_the_supplied_time_instead_of_the_wall_clock(pool: PgPool) {
        let now = DateTime::from_timestamp(1_600_000_000, 0).unwrap();
        let feed = upsert_feed_by_url(&pool, "https://example.com/recency")
            .await
            .unwrap();
        let mut ids = Vec::new();
        // The older item gets the larger ID, so a tie puts it first.
        for (key, date) in [("recent", now), ("older", now - Duration::hours(48))] {
            let item = insert_feed_item(
                &pool,
                feed.id,
                key,
                "Article",
                &format!("https://example.com/{key}"),
            )
            .await
            .unwrap();
            insert_feed_item_detail(&pool, item.id, "", "", "", date)
                .await
                .unwrap();
            store_item_embeddings(
                &pool,
                "test-model",
                &[ItemEmbedding {
                    feed_item_id: item.id,
                    content_hash: "h".into(),
                    embedding: vec![1.0, 0.0],
                }],
            )
            .await
            .unwrap();
            ids.push(item.id);
        }
        let mut state = SearchState {
            pool,
            embedder: Arc::new(FixedEmbedder),
            model: "test-model".into(),
            embeddings: Arc::new(EmbeddingIndex::new("test-model")),
            min_similarity: 0.3,
            recency: RecencyBoost {
                weight: 0.5,
                half_life_hours: 24.0,
            },
        };
        let query = LatestItemsQuery {
            limit: None,
            offset: None,
            feed_id: Some(feed.id),
            q: Some(" news ".into()),
            mode: SearchMode::Semantic,
        };

        let recent_first = search_items(&state, &query, now).await.unwrap();
        assert_eq!(
            recent_first.iter().map(|item| item.id).collect::<Vec<_>>(),
            ids
        );

        // At this age both multipliers round to the same floor, so the ID breaks the tie.
        let tied = search_items(&state, &query, now + Duration::hours(1000))
            .await
            .unwrap();
        assert_eq!(
            tied.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![ids[1], ids[0]]
        );

        state.recency = RecencyBoost::NONE;
        let unboosted = search_items(&state, &query, now).await.unwrap();
        assert_eq!(
            unboosted.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![ids[1], ids[0]]
        );
    }
}
