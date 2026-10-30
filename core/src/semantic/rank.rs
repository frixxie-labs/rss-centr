//! Ranking search results by relevance, keywords and recency.

use chrono::{DateTime, Utc};

/// Semantic-only matches must reach at least this [`RelevanceScale`] relevance.
pub const MIN_RELEVANCE: f32 = 0.5;

/// Places a query's similarities on a common scale: the best match has
/// relevance 1 and an average item 0.
///
/// Embedding models give unrelated texts a substantial similarity
/// (`embeddinggemma`: about 0.55-0.65, against 0.75-0.8 for strong matches),
/// and the level shifts with query length. Raw similarities therefore say
/// little on their own; only how far an item stands out from the rest does.
///
/// The mean is taken over all embedded items, not just the candidates of a
/// nearest-neighbour search, which are by construction above average.
struct RelevanceScale {
    mean: f32,
    spread: f32,
}

impl RelevanceScale {
    fn new(mean: f32, best: f32) -> Self {
        Self {
            mean,
            spread: best - mean,
        }
    }

    /// `similarity` as a fraction of the way from the mean to the best match,
    /// clamped to `0..=1`. All items are fully relevant when they score alike.
    fn relevance(&self, similarity: f32) -> f32 {
        if self.spread.is_nan() || self.spread <= f32::EPSILON {
            return 1.0;
        }
        ((similarity - self.mean) / self.spread).clamp(0.0, 1.0)
    }
}

/// How strongly ranked search favours recent items.
///
/// An item's relevance is multiplied by `(1 - weight) + weight * decay`, where
/// `decay` halves every `half_life_hours`. A brand-new item keeps its full
/// relevance while a very old one keeps `1 - weight` of it, so recency decides
/// between similarly relevant items without lifting unrelated ones.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecencyBoost {
    pub weight: f32,
    pub half_life_hours: f32,
}

impl RecencyBoost {
    pub const DEFAULT_WEIGHT: f32 = 0.3;
    pub const DEFAULT_HALF_LIFE_HOURS: f32 = 168.0;

    /// Ranks by relevance alone.
    pub const NONE: Self = Self {
        weight: 0.0,
        half_life_hours: Self::DEFAULT_HALF_LIFE_HOURS,
    };

    /// Multiplier for an item dated `date`, as of `now`.
    pub fn factor(&self, date: DateTime<Utc>, now: DateTime<Utc>) -> f32 {
        let weight = self.weight.clamp(0.0, 1.0);
        if weight == 0.0 {
            return 1.0;
        }
        let age_hours = (now - date).num_seconds() as f32 / 3600.0;
        (1.0 - weight) + weight * recency_decay(age_hours, self.half_life_hours)
    }
}

impl Default for RecencyBoost {
    fn default() -> Self {
        Self {
            weight: Self::DEFAULT_WEIGHT,
            half_life_hours: Self::DEFAULT_HALF_LIFE_HOURS,
        }
    }
}

/// Exponential decay that halves every `half_life_hours`. Future dates count
/// as brand new; a non-positive half-life disables decay.
pub fn recency_decay(age_hours: f32, half_life_hours: f32) -> f32 {
    if half_life_hours <= 0.0 || !half_life_hours.is_finite() || !age_hours.is_finite() {
        return 1.0;
    }
    0.5_f32.powf(age_hours.max(0.0) / half_life_hours)
}

/// The date used for recency ranking: the publish date, unless missing or
/// later than when the item was first seen.
pub fn item_date(published_at: Option<DateTime<Utc>>, inserted_at: DateTime<Utc>) -> DateTime<Utc> {
    published_at.map_or(inserted_at, |p| p.min(inserted_at))
}

/// The average of `similarities`, or `None` if there are none.
pub fn mean_similarity(similarities: impl IntoIterator<Item = f32>) -> Option<f32> {
    let (count, sum) = similarities
        .into_iter()
        .fold((0usize, 0.0f64), |(count, sum), s| {
            (count + 1, sum + f64::from(s))
        });
    (count > 0).then(|| (sum / count as f64) as f32)
}

/// Relevance added to items whose text matches the search keywords in hybrid
/// search, on the [`RelevanceScale`] where the best match scores 1.
pub const KEYWORD_BONUS: f32 = 0.2;

/// Rank items for search by meaning, nudged by keyword matches and recency.
///
/// `similarities` holds the similarity of every embedded candidate,
/// `mean_similarity` the query's average similarity over all embedded items
/// (see [`mean_similarity`]) and `keyword_ids` the items matching the keywords
/// (empty for semantic-only search). Similarities are converted to relevance,
/// see [`RelevanceScale`].
/// Keyword matches score their relevance plus [`KEYWORD_BONUS`], so they are
/// always included but cannot outrank clearly stronger meaning matches. Other
/// items must reach [`MIN_RELEVANCE`] and a similarity of `min_similarity`.
/// Each score is then multiplied by `factor(id)`, e.g.
/// [`RecencyBoost::factor`]. Ties are broken by the newest (largest) id.
pub fn hybrid_rank(
    similarities: &[(i64, f32)],
    mean_similarity: f32,
    keyword_ids: &[i64],
    min_similarity: f32,
    limit: usize,
    factor: impl Fn(i64) -> f32,
) -> Vec<i64> {
    use std::collections::{HashMap, HashSet};

    let mut best: HashMap<i64, f32> = HashMap::new();
    for &(id, similarity) in similarities {
        let entry = best.entry(id).or_insert(f32::NEG_INFINITY);
        *entry = entry.max(similarity);
    }
    let keyword: HashSet<i64> = keyword_ids.iter().copied().collect();
    let top = best.values().copied().fold(f32::NEG_INFINITY, f32::max);
    let scale = RelevanceScale::new(mean_similarity, top);

    let mut scored: Vec<(i64, f32)> = best
        .iter()
        .filter(|(id, similarity)| **similarity >= min_similarity && !keyword.contains(id))
        .map(|(id, similarity)| (*id, scale.relevance(*similarity)))
        .filter(|(_, relevance)| *relevance >= MIN_RELEVANCE)
        .collect();
    scored.extend(keyword.iter().map(|id| {
        let relevance = best.get(id).map_or(0.0, |s| scale.relevance(*s));
        (*id, relevance + KEYWORD_BONUS)
    }));
    for (id, score) in &mut scored {
        *score *= factor(*id);
    }
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    scored.into_iter().take(limit).map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: f32 = 0.2;

    /// `items` among 20 unrelated items scoring 0.55-0.60, the typical
    /// spread of `embeddinggemma` similarities.
    fn with_background(items: &[(i64, f32)]) -> Vec<(i64, f32)> {
        let mut scores: Vec<(i64, f32)> = (0..20)
            .map(|i| (100 + i, 0.55 + i as f32 * 0.0025))
            .collect();
        scores.extend_from_slice(items);
        scores
    }

    fn mean(scores: &[(i64, f32)]) -> f32 {
        mean_similarity(scores.iter().map(|(_, s)| *s)).unwrap_or(0.0)
    }

    #[test]
    fn mean_similarity_averages_and_is_none_when_empty() {
        assert_eq!(mean_similarity([0.2, 0.4, 0.9]), Some(0.5));
        assert_eq!(mean_similarity([]), None);
    }

    #[test]
    fn relevance_runs_from_the_mean_to_the_best_similarity() {
        let scale = RelevanceScale::new(0.5, 1.0);
        assert!(close(scale.relevance(1.0), 1.0));
        assert!(close(scale.relevance(0.75), 0.5));
        assert!(close(scale.relevance(0.5), 0.0));
        assert!(
            close(scale.relevance(0.0), 0.0),
            "below the mean clamps to 0"
        );
    }

    #[test]
    fn relevance_is_full_when_all_similarities_are_equal() {
        assert!(close(RelevanceScale::new(0.3, 0.3).relevance(0.3), 1.0));
    }

    #[test]
    fn hybrid_drops_matches_close_to_the_typical_similarity() {
        let scores = with_background(&[(1, 0.80), (2, 0.75), (3, 0.65)]);
        let ranked = hybrid_rank(&scores, mean(&scores), &[], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_measures_relevance_from_the_corpus_mean_not_the_candidates() {
        // Nearest-neighbour candidates are all well above the corpus mean.
        let candidates = [(1, 0.80), (2, 0.75)];
        assert_eq!(
            hybrid_rank(&candidates, 0.58, &[], MIN, 10, |_| 1.0),
            vec![1, 2]
        );
        assert_eq!(
            hybrid_rank(&candidates, mean(&candidates), &[], MIN, 10, |_| 1.0),
            vec![1],
            "against the candidates' own mean, the second looks irrelevant"
        );
    }

    #[test]
    fn hybrid_applies_the_absolute_minimum_similarity() {
        let ranked = hybrid_rank(
            &[(1, 0.15), (2, 0.05), (3, 0.04)],
            0.08,
            &[3],
            MIN,
            10,
            |_| 1.0,
        );
        assert_eq!(ranked, vec![3], "only the keyword match is kept");
    }

    #[test]
    fn hybrid_strong_meaning_match_outranks_weak_match_with_keywords() {
        // The "AMP with Carter Schultz" case: item 2 mentions the words but is
        // only loosely related; item 1 is clearly about the topic.
        let scores = with_background(&[(1, 0.80), (2, 0.62)]);
        let ranked = hybrid_rank(&scores, mean(&scores), &[2], MIN, 10, |_| 1.0);
        assert_eq!(
            ranked,
            vec![1, 2],
            "weak keyword matches are kept, but last"
        );
    }

    #[test]
    fn hybrid_keyword_match_breaks_near_ties() {
        let scores = with_background(&[(1, 0.80), (2, 0.78)]);
        let ranked = hybrid_rank(&scores, mean(&scores), &[2], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![2, 1]);
    }

    #[test]
    fn hybrid_includes_keyword_matches_without_embeddings() {
        let scores = with_background(&[(1, 0.80)]);
        let ranked = hybrid_rank(&scores, mean(&scores), &[9], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![1, 9]);
        assert_eq!(hybrid_rank(&[], 0.0, &[9], MIN, 10, |_| 1.0), vec![9]);
    }

    #[test]
    fn hybrid_respects_limit_and_breaks_ties_by_newest_id() {
        let ranked = hybrid_rank(&[(1, 0.3), (5, 0.3), (3, 0.3)], 0.3, &[], MIN, 2, |_| 1.0);
        assert_eq!(ranked, vec![5, 3]);
        assert!(hybrid_rank(&[], 0.0, &[], MIN, 10, |_| 1.0).is_empty());
    }

    fn hours_ago(hours: i64) -> DateTime<Utc> {
        now() - chrono::Duration::hours(hours)
    }

    fn now() -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000, 0).unwrap()
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn recency_decay_halves_every_half_life() {
        assert!(close(recency_decay(0.0, 168.0), 1.0));
        assert!(close(recency_decay(168.0, 168.0), 0.5));
        assert!(close(recency_decay(336.0, 168.0), 0.25));
    }

    #[test]
    fn recency_decay_treats_future_dates_as_new_and_ignores_bad_half_life() {
        assert!(close(recency_decay(-10.0, 168.0), 1.0));
        assert!(close(recency_decay(500.0, 0.0), 1.0));
        assert!(close(recency_decay(500.0, -1.0), 1.0));
    }

    #[test]
    fn recency_factor_ranges_from_one_minus_weight_to_one() {
        let boost = RecencyBoost {
            weight: 0.3,
            half_life_hours: 168.0,
        };
        assert!(close(boost.factor(now(), now()), 1.0));
        assert!(close(boost.factor(hours_ago(168), now()), 0.85));
        assert!(close(boost.factor(hours_ago(100_000), now()), 0.7));
        assert!(close(
            RecencyBoost::NONE.factor(hours_ago(100_000), now()),
            1.0
        ));
    }

    #[test]
    fn item_date_prefers_publish_date_but_not_after_insertion() {
        let inserted = hours_ago(1);
        assert_eq!(item_date(None, inserted), inserted);
        assert_eq!(item_date(Some(hours_ago(5)), inserted), hours_ago(5));
        assert_eq!(item_date(Some(now()), inserted), inserted);
    }

    /// Recency factor for items of the given ages in hours; other items are new.
    fn by_age(ages: &[(i64, i64)]) -> impl Fn(i64) -> f32 {
        let ages: std::collections::HashMap<i64, i64> = ages.iter().copied().collect();
        move |id| {
            let age = ages.get(&id).copied().unwrap_or(0);
            RecencyBoost::default().factor(hours_ago(age), now())
        }
    }

    #[test]
    fn hybrid_recent_item_outranks_slightly_more_similar_old_one() {
        let scores = with_background(&[(1, 0.80), (2, 0.78)]);
        let ranked = hybrid_rank(
            &scores,
            mean(&scores),
            &[],
            MIN,
            10,
            by_age(&[(1, 24 * 365), (2, 2)]),
        );
        assert_eq!(ranked, vec![2, 1]);
    }

    #[test]
    fn hybrid_much_more_relevant_old_item_beats_weak_recent_one() {
        // "getting fired from my job": a year-old podcast episode about being
        // fired must beat today's loosely related news.
        let scores = with_background(&[(1, 0.79), (2, 0.70)]);
        let ranked = hybrid_rank(
            &scores,
            mean(&scores),
            &[],
            MIN,
            10,
            by_age(&[(1, 24 * 365), (2, 0)]),
        );
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_recency_does_not_rescue_items_below_the_floor() {
        let scores = with_background(&[(1, 0.80), (2, 0.65)]);
        let ranked = hybrid_rank(
            &scores,
            mean(&scores),
            &[],
            MIN,
            10,
            by_age(&[(1, 24 * 365), (2, 0)]),
        );
        assert_eq!(ranked, vec![1]);
    }

    quickcheck::quickcheck! {
        fn prop_hybrid_output_is_unique_bounded_and_keeps_keyword_matches(scores: Vec<(i64, i8)>, keyword: Vec<i64>, limit: u8) -> bool {
            use std::collections::HashSet;
            let scores: Vec<(i64, f32)> = scores
                .into_iter()
                .map(|(id, s)| (id, f32::from(s) / 128.0))
                .collect();
            let limit = usize::from(limit);
            let ranked = hybrid_rank(&scores, mean(&scores), &keyword, MIN, limit, |_| 1.0);
            let candidates: HashSet<i64> = scores.iter().map(|(id, _)| *id).chain(keyword.iter().copied()).collect();
            let keyword: HashSet<i64> = keyword.into_iter().collect();
            let unique: HashSet<i64> = ranked.iter().copied().collect();
            unique.len() == ranked.len()
                && ranked.len() <= limit
                && unique.is_subset(&candidates)
                && (candidates.len() > limit || keyword.is_subset(&unique))
        }

        fn prop_relevance_is_between_zero_and_one(scores: Vec<i8>) -> bool {
            let scores: Vec<f32> = scores.into_iter().map(|s| f32::from(s) / 128.0).collect();
            let best = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
            let scale = RelevanceScale::new(mean_similarity(scores.iter().copied()).unwrap_or(0.0), best);
            scores.iter().all(|s| (0.0..=1.0 + 1e-6).contains(&scale.relevance(*s)))
        }
    }
}
