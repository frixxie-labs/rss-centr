use std::{future::Future, time::Duration};

use metrics::histogram;
use tokio::time::Instant;
use tracing::info;

use super::items::RankedMode;

/// A stage of a ranked (semantic or hybrid) search, see [`SearchTimings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchStage {
    /// Embedding the query (instant for cached queries).
    Embed,
    /// Loading the embedding centroids (only until they are first loaded).
    Refresh,
    /// The nearest-neighbour query.
    Scan,
    /// The keyword query of hybrid search.
    Keyword,
    /// Ranking by relevance, keywords and recency.
    Rank,
    /// Reading the returned items.
    Load,
}

impl SearchStage {
    pub const ALL: [Self; 6] = [
        Self::Embed,
        Self::Refresh,
        Self::Scan,
        Self::Keyword,
        Self::Rank,
        Self::Load,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Embed => "embed",
            Self::Refresh => "refresh",
            Self::Scan => "scan",
            Self::Keyword => "keyword",
            Self::Rank => "rank",
            Self::Load => "load",
        }
    }
}

/// Counts logged with a search's timings.
#[derive(Debug, Default, Clone, Copy)]
pub struct SearchCounts {
    /// Embeddings of the model, as of the last centroid load.
    pub indexed: usize,
    /// Candidates scored by similarity, after the feed filter.
    pub scored: usize,
    /// Items matching the keywords (hybrid search).
    pub keyword_matches: usize,
    /// Items returned.
    pub results: usize,
}

/// Times the stages of one ranked search. Each stage is recorded in the
/// `search_stage` histogram (labels `mode`, `stage`), and [`Self::finish`]
/// logs all of them with the total in one line.
pub struct SearchTimings {
    mode: RankedMode,
    started: Instant,
    durations: [Option<Duration>; SearchStage::ALL.len()],
}

impl SearchTimings {
    pub(crate) fn new(mode: RankedMode) -> Self {
        Self {
            mode,
            started: Instant::now(),
            durations: [None; SearchStage::ALL.len()],
        }
    }

    /// Await `future`, adding the time it takes to `stage`.
    pub async fn time<F: Future>(&mut self, stage: SearchStage, future: F) -> F::Output {
        let started = Instant::now();
        let output = future.await;
        let elapsed = started.elapsed();
        histogram!("search_stage", "mode" => self.mode.name(), "stage" => stage.name())
            .record(elapsed);
        let total = &mut self.durations[stage as usize];
        *total = Some(total.unwrap_or_default() + elapsed);
        output
    }

    pub fn duration(&self, stage: SearchStage) -> Option<Duration> {
        self.durations[stage as usize]
    }

    /// Time since the search started.
    pub fn total(&self) -> Duration {
        self.started.elapsed()
    }

    /// Record the total and log every stage. Stages that did not run are
    /// left out of the log line.
    pub fn finish(self, counts: SearchCounts) {
        let total = self.total();
        histogram!("search_stage", "mode" => self.mode.name(), "stage" => "total").record(total);
        let ms = |stage| self.duration(stage).map(millis);
        info!(
            mode = self.mode.name(),
            total_ms = millis(total),
            embed_ms = ms(SearchStage::Embed),
            refresh_ms = ms(SearchStage::Refresh),
            scan_ms = ms(SearchStage::Scan),
            keyword_ms = ms(SearchStage::Keyword),
            rank_ms = ms(SearchStage::Rank),
            load_ms = ms(SearchStage::Load),
            indexed = counts.indexed,
            scored = counts.scored,
            keyword_matches = counts.keyword_matches,
            results = counts.results,
            "search finished"
        );
    }
}

/// Milliseconds with two decimals, readable in logs.
fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 100_000.0).round() / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn timings_return_the_output_and_record_the_stage() {
        let mut timings = SearchTimings::new(RankedMode::Hybrid);
        let output = timings
            .time(SearchStage::Scan, async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                42
            })
            .await;
        assert_eq!(output, 42);
        assert!(timings.duration(SearchStage::Scan).unwrap() >= Duration::from_millis(5));
        assert_eq!(timings.duration(SearchStage::Keyword), None, "not run");
    }

    #[tokio::test]
    async fn timings_add_up_repeated_stages() {
        let mut timings = SearchTimings::new(RankedMode::Semantic);
        for _ in 0..2 {
            timings
                .time(
                    SearchStage::Load,
                    tokio::time::sleep(Duration::from_millis(3)),
                )
                .await;
        }
        assert!(timings.duration(SearchStage::Load).unwrap() >= Duration::from_millis(6));
    }

    #[tokio::test]
    async fn total_covers_every_stage() {
        let mut timings = SearchTimings::new(RankedMode::Semantic);
        timings
            .time(
                SearchStage::Embed,
                tokio::time::sleep(Duration::from_millis(2)),
            )
            .await;
        timings
            .time(
                SearchStage::Rank,
                tokio::time::sleep(Duration::from_millis(2)),
            )
            .await;
        let stages = timings.duration(SearchStage::Embed).unwrap()
            + timings.duration(SearchStage::Rank).unwrap();
        assert!(timings.total() >= stages);
    }

    #[test]
    fn stage_names_are_distinct_metric_labels() {
        let names: std::collections::HashSet<&str> =
            SearchStage::ALL.iter().map(|s| s.name()).collect();
        assert_eq!(names.len(), SearchStage::ALL.len());
    }
}
