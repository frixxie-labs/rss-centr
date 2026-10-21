//! Pure building blocks for semantic search, shared by the backend and worker.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

/// Upper bound on the number of characters sent to the embedding model per item.
pub const MAX_EMBEDDING_INPUT_CHARS: usize = 2000;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct EmbeddingJob {
    pub feed_item_id: i64,
    pub input: String,
    pub content_hash: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ClaimEmbeddingsResponse {
    pub model: String,
    pub jobs: Vec<EmbeddingJob>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct ItemEmbedding {
    pub feed_item_id: i64,
    pub content_hash: String,
    pub embedding: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StoreEmbeddingsRequest {
    pub model: String,
    pub embeddings: Vec<ItemEmbedding>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct StoreEmbeddingsResult {
    pub stored: u64,
}

/// Build the document text embedded for a feed item.
///
/// HTML is stripped, whitespace collapsed, duplicated summary/content is
/// dropped and the result is truncated to [`MAX_EMBEDDING_INPUT_CHARS`].
pub fn embedding_input(title: &str, summary: Option<&str>, content: Option<&str>) -> String {
    let title = clean_text(title);
    let summary = summary.map(clean_text).unwrap_or_default();
    let content = content.map(clean_text).unwrap_or_default();

    let mut text = summary;
    if !content.is_empty() && !text.contains(&content) {
        if content.contains(&text) {
            text = content;
        } else {
            text.push(' ');
            text.push_str(&content);
        }
    }

    let title = if title.is_empty() { "none" } else { &title };
    let input = format!("title: {title} | text: {}", text.trim());
    truncate_chars(&input, MAX_EMBEDDING_INPUT_CHARS)
}

/// Build the text embedded for a search query.
pub fn query_input(query: &str) -> String {
    format!("task: search result | query: {}", clean_text(query))
}

/// Stable (cross-version, cross-platform) FNV-1a 64-bit hash, hex encoded.
pub fn content_hash(input: &str) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Scale a vector to unit length. Returns `None` for empty, zero or non-finite vectors.
pub fn normalize(vector: &[f32]) -> Option<Vec<f32>> {
    if vector.is_empty() || vector.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm == 0.0 || !norm.is_finite() {
        return None;
    }
    Some(vector.iter().map(|v| v / norm).collect())
}

/// Number of independent partial sums in [`dot`]. Float addition is not
/// associative, so a single running sum is a serial chain the compiler cannot
/// vectorize; separate accumulators let it use SIMD lanes.
const DOT_LANES: usize = 16;

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    let (a_chunks, a_rest) = a[..len].as_chunks::<DOT_LANES>();
    let (b_chunks, b_rest) = b[..len].as_chunks::<DOT_LANES>();
    let mut acc = [0.0f32; DOT_LANES];
    for (x, y) in a_chunks.iter().zip(b_chunks) {
        for i in 0..DOT_LANES {
            acc[i] += x[i] * y[i];
        }
    }
    let rest: f32 = a_rest.iter().zip(b_rest).map(|(x, y)| x * y).sum();
    acc.iter().sum::<f32>() + rest
}

/// Return the `k` candidates most similar to `query` (by dot product, i.e.
/// cosine similarity for normalized vectors), highest first. Ties are broken
/// by the newest (largest) id. Candidates whose dimension differs from the
/// query are ignored.
pub fn top_k<'a, I>(query: &[f32], candidates: I, k: usize) -> Vec<(i64, f32)>
where
    I: IntoIterator<Item = (i64, &'a [f32])>,
{
    let mut scored: Vec<(i64, f32)> = candidates
        .into_iter()
        .filter(|(_, vector)| vector.len() == query.len())
        .map(|(id, vector)| (id, dot(query, vector)))
        .filter(|(_, score)| score.is_finite())
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
    scored.truncate(k);
    scored
}

/// Semantic-only matches must reach at least this [`RelevanceScale`] relevance.
pub const MIN_RELEVANCE: f32 = 0.5;

/// Places a query's similarities on a common scale: the best match has
/// relevance 1 and an average item 0.
///
/// Embedding models give unrelated texts a substantial similarity
/// (`embeddinggemma`: about 0.55-0.65, against 0.75-0.8 for strong matches),
/// and the level shifts with query length. Raw similarities therefore say
/// little on their own; only how far an item stands out from the rest does.
struct RelevanceScale {
    mean: f32,
    spread: f32,
}

impl RelevanceScale {
    fn new(similarities: impl IntoIterator<Item = f32>) -> Self {
        let (mut count, mut sum, mut best) = (0usize, 0.0f64, f32::NEG_INFINITY);
        for similarity in similarities {
            count += 1;
            sum += f64::from(similarity);
            best = best.max(similarity);
        }
        let mean = if count == 0 {
            0.0
        } else {
            (sum / count as f64) as f32
        };
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

/// Relevance added to items whose text matches the search keywords in hybrid
/// search, on the [`RelevanceScale`] where the best match scores 1.
pub const KEYWORD_BONUS: f32 = 0.2;

/// Rank items for search by meaning, nudged by keyword matches and recency.
///
/// `similarities` holds the similarity of every embedded candidate and
/// `keyword_ids` the items matching the keywords (empty for semantic-only
/// search). Similarities are converted to relevance, see [`RelevanceScale`].
/// Keyword matches score their relevance plus [`KEYWORD_BONUS`], so they are
/// always included but cannot outrank clearly stronger meaning matches. Other
/// items must reach [`MIN_RELEVANCE`] and a similarity of `min_similarity`.
/// Each score is then multiplied by `factor(id)`, e.g.
/// [`RecencyBoost::factor`]. Ties are broken by the newest (largest) id.
pub fn hybrid_rank(
    similarities: &[(i64, f32)],
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
    let scale = RelevanceScale::new(best.values().copied());

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

fn clean_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => {
                in_tag = true;
                out.push(' ');
            }
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    let decoded = out
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedding_input_combines_title_summary_and_content() {
        let input = embedding_input("Rust 2.0", Some("A summary."), Some("Body text."));
        assert_eq!(input, "title: Rust 2.0 | text: A summary. Body text.");
    }

    #[test]
    fn embedding_input_strips_html_and_collapses_whitespace() {
        let input = embedding_input(
            "  Hello <b>world</b> ",
            Some("<p>Fish &amp; chips</p>\n\n<p>are   great</p>"),
            None,
        );
        assert_eq!(input, "title: Hello world | text: Fish & chips are great");
    }

    #[test]
    fn embedding_input_does_not_repeat_content_contained_in_summary() {
        let input = embedding_input("T", Some("Same text"), Some("<p>Same text</p>"));
        assert_eq!(input, "title: T | text: Same text");

        let input = embedding_input("T", Some("Short"), Some("Short and longer"));
        assert_eq!(input, "title: T | text: Short and longer");
    }

    #[test]
    fn embedding_input_handles_missing_parts() {
        assert_eq!(embedding_input("", None, None), "title: none | text: ");
        assert_eq!(embedding_input("Only", None, None), "title: Only | text: ");
    }

    #[test]
    fn embedding_input_truncates_on_char_boundary() {
        let long = "å".repeat(MAX_EMBEDDING_INPUT_CHARS * 2);
        let input = embedding_input("T", Some(&long), None);
        assert_eq!(input.chars().count(), MAX_EMBEDDING_INPUT_CHARS);
    }

    #[test]
    fn query_input_uses_search_prompt() {
        assert_eq!(
            query_input("  electric   cars "),
            "task: search result | query: electric cars"
        );
    }

    #[test]
    fn content_hash_is_stable_and_sensitive_to_changes() {
        assert_eq!(content_hash("abc"), content_hash("abc"));
        assert_ne!(content_hash("abc"), content_hash("abd"));
        // Pinned so stored hashes stay valid across releases.
        assert_eq!(content_hash(""), "cbf29ce484222325");
        assert_eq!(content_hash("a"), "af63dc4c8601ec8c");
    }

    #[test]
    fn normalize_produces_unit_vectors() {
        let v = normalize(&[3.0, 4.0]).unwrap();
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn normalize_rejects_degenerate_vectors() {
        assert_eq!(normalize(&[]), None);
        assert_eq!(normalize(&[0.0, 0.0]), None);
        assert_eq!(normalize(&[f32::NAN, 1.0]), None);
        assert_eq!(normalize(&[f32::INFINITY, 1.0]), None);
    }

    fn naive_dot(a: &[f32], b: &[f32]) -> f64 {
        a.iter()
            .zip(b)
            .map(|(x, y)| f64::from(*x) * f64::from(*y))
            .sum()
    }

    fn pseudo_random_vector(len: usize, seed: u32) -> Vec<f32> {
        let mut state = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                (state as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect()
    }

    #[test]
    fn dot_matches_naive_sum_around_chunk_boundaries() {
        for len in 0..=70 {
            let a = pseudo_random_vector(len, 1);
            let b = pseudo_random_vector(len, 2);
            let expected = naive_dot(&a, &b);
            let actual = f64::from(dot(&a, &b));
            assert!(
                (actual - expected).abs() < 1e-4,
                "len={len}: {actual} != {expected}"
            );
        }
    }

    #[test]
    fn dot_ignores_extra_elements_of_the_longer_vector() {
        let a = pseudo_random_vector(40, 3);
        let b = pseudo_random_vector(23, 4);
        assert!((f64::from(dot(&a, &b)) - naive_dot(&a[..23], &b)).abs() < 1e-4);
        assert!((f64::from(dot(&b, &a)) - naive_dot(&b, &a[..23])).abs() < 1e-4);
    }

    #[test]
    fn dot_of_unit_vector_with_itself_is_one() {
        let v = normalize(&pseudo_random_vector(768, 5)).unwrap();
        assert!((dot(&v, &v) - 1.0).abs() < 1e-5);
    }

    /// Compare `dot` with a plain sequential sum on an index-sized workload:
    /// `cargo test -p rss-centr-core --release bench_dot -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn bench_dot() {
        use std::hint::black_box;
        use std::time::Instant;

        fn sequential(a: &[f32], b: &[f32]) -> f32 {
            a.iter().zip(b).map(|(x, y)| x * y).sum()
        }

        const DIM: usize = 768;
        const ITEMS: usize = 50_000;
        const ROUNDS: u32 = 10;
        let query = pseudo_random_vector(DIM, 0);
        let index: Vec<Vec<f32>> = (1..=ITEMS as u32)
            .map(|seed| pseudo_random_vector(DIM, seed))
            .collect();

        let time = |f: fn(&[f32], &[f32]) -> f32| {
            let start = Instant::now();
            let mut total = 0.0;
            for _ in 0..ROUNDS {
                for v in &index {
                    total += f(black_box(&query), black_box(v));
                }
            }
            black_box(total);
            start.elapsed() / ROUNDS
        };

        let sequential_time = time(sequential);
        let dot_time = time(dot);
        println!(
            "{ITEMS} x {DIM}: sequential {sequential_time:?}, dot {dot_time:?} ({:.1}x)",
            sequential_time.as_secs_f64() / dot_time.as_secs_f64()
        );
    }

    #[test]
    fn top_k_orders_by_similarity() {
        let a = [1.0, 0.0];
        let b = [0.6, 0.8];
        let c = [0.0, 1.0];
        let candidates = vec![(1, &c[..]), (2, &a[..]), (3, &b[..])];
        let result = top_k(&[1.0, 0.0], candidates, 10);
        let ids: Vec<i64> = result.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![2, 3, 1]);
    }

    #[test]
    fn top_k_respects_k_and_breaks_ties_by_newest_id() {
        let v = [1.0, 0.0];
        let candidates = vec![(1, &v[..]), (5, &v[..]), (3, &v[..])];
        let result = top_k(&[1.0, 0.0], candidates, 2);
        assert_eq!(
            result.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
            vec![5, 3]
        );
    }

    #[test]
    fn top_k_skips_dimension_mismatches() {
        let ok = [1.0, 0.0];
        let bad = [1.0, 0.0, 0.0];
        let result = top_k(&[1.0, 0.0], vec![(1, &bad[..]), (2, &ok[..])], 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].0, 2);
    }

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

    #[test]
    fn relevance_runs_from_the_mean_to_the_best_similarity() {
        let scale = RelevanceScale::new([1.0, 0.5, 0.0]);
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
        assert!(close(RelevanceScale::new([0.3]).relevance(0.3), 1.0));
        assert!(close(RelevanceScale::new([0.3, 0.3]).relevance(0.3), 1.0));
    }

    #[test]
    fn hybrid_drops_matches_close_to_the_typical_similarity() {
        let scores = with_background(&[(1, 0.80), (2, 0.75), (3, 0.65)]);
        let ranked = hybrid_rank(&scores, &[], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_applies_the_absolute_minimum_similarity() {
        let ranked = hybrid_rank(&[(1, 0.15), (2, 0.05), (3, 0.04)], &[3], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![3], "only the keyword match is kept");
    }

    #[test]
    fn hybrid_strong_meaning_match_outranks_weak_match_with_keywords() {
        // The "AMP with Carter Schultz" case: item 2 mentions the words but is
        // only loosely related; item 1 is clearly about the topic.
        let scores = with_background(&[(1, 0.80), (2, 0.62)]);
        let ranked = hybrid_rank(&scores, &[2], MIN, 10, |_| 1.0);
        assert_eq!(
            ranked,
            vec![1, 2],
            "weak keyword matches are kept, but last"
        );
    }

    #[test]
    fn hybrid_keyword_match_breaks_near_ties() {
        let scores = with_background(&[(1, 0.80), (2, 0.78)]);
        let ranked = hybrid_rank(&scores, &[2], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![2, 1]);
    }

    #[test]
    fn hybrid_includes_keyword_matches_without_embeddings() {
        let scores = with_background(&[(1, 0.80)]);
        let ranked = hybrid_rank(&scores, &[9], MIN, 10, |_| 1.0);
        assert_eq!(ranked, vec![1, 9]);
        assert_eq!(hybrid_rank(&[], &[9], MIN, 10, |_| 1.0), vec![9]);
    }

    #[test]
    fn hybrid_respects_limit_and_breaks_ties_by_newest_id() {
        let ranked = hybrid_rank(&[(1, 0.3), (5, 0.3), (3, 0.3)], &[], MIN, 2, |_| 1.0);
        assert_eq!(ranked, vec![5, 3]);
        assert!(hybrid_rank(&[], &[], MIN, 10, |_| 1.0).is_empty());
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
        let ranked = hybrid_rank(&scores, &[], MIN, 10, by_age(&[(1, 24 * 365), (2, 2)]));
        assert_eq!(ranked, vec![2, 1]);
    }

    #[test]
    fn hybrid_much_more_relevant_old_item_beats_weak_recent_one() {
        // "getting fired from my job": a year-old podcast episode about being
        // fired must beat today's loosely related news.
        let scores = with_background(&[(1, 0.79), (2, 0.70)]);
        let ranked = hybrid_rank(&scores, &[], MIN, 10, by_age(&[(1, 24 * 365), (2, 0)]));
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_recency_does_not_rescue_items_below_the_floor() {
        let scores = with_background(&[(1, 0.80), (2, 0.65)]);
        let ranked = hybrid_rank(&scores, &[], MIN, 10, by_age(&[(1, 24 * 365), (2, 0)]));
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
            let ranked = hybrid_rank(&scores, &keyword, MIN, limit, |_| 1.0);
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
            let scale = RelevanceScale::new(scores.iter().copied());
            scores.iter().all(|s| (0.0..=1.0 + 1e-6).contains(&scale.relevance(*s)))
        }


        fn prop_normalize_yields_unit_length(v: Vec<i16>) -> bool {
            let v: Vec<f32> = v.into_iter().map(f32::from).collect();
            match normalize(&v) {
                Some(n) => (n.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-3,
                None => v.iter().all(|x| *x == 0.0),
            }
        }

        fn prop_dot_matches_naive_sum(pairs: Vec<(i8, i8)>) -> bool {
            let a: Vec<f32> = pairs.iter().map(|(x, _)| f32::from(*x) / 128.0).collect();
            let b: Vec<f32> = pairs.iter().map(|(_, y)| f32::from(*y) / 128.0).collect();
            (f64::from(dot(&a, &b)) - naive_dot(&a, &b)).abs() < 1e-3
        }

        fn prop_top_k_is_sorted_and_bounded(scores: Vec<i16>, k: u8) -> bool {
            let vectors: Vec<[f32; 1]> = scores.iter().map(|s| [f32::from(*s)]).collect();
            let result = top_k(&[1.0], vectors.iter().enumerate().map(|(i, v)| (i as i64, &v[..])), usize::from(k));
            result.len() == scores.len().min(usize::from(k))
                && result.windows(2).all(|w| w[0].1 >= w[1].1)
        }
    }
}
