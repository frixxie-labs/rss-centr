//! Pure building blocks for semantic search, shared by the backend and worker.

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

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
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

/// Semantic matches must score at least this fraction of the best match.
/// Long queries score higher across the board than short ones, so a fixed
/// minimum alone lets many loosely related items through for long queries.
pub const RELATIVE_SIMILARITY: f32 = 0.6;

/// Lowest similarity a semantic-only match may have: the larger of
/// `min_similarity` and [`RELATIVE_SIMILARITY`] of the best match.
pub fn similarity_floor(similarities: &[(i64, f32)], min_similarity: f32) -> f32 {
    let best = similarities
        .iter()
        .map(|(_, score)| *score)
        .fold(f32::NEG_INFINITY, f32::max);
    min_similarity.max(best * RELATIVE_SIMILARITY)
}

/// Score added to items whose text matches the search keywords in hybrid search.
pub const KEYWORD_BONUS: f32 = 0.1;

/// Rank items for hybrid search by meaning, nudged by keyword matches.
///
/// `similarities` holds the similarity of every embedded candidate and
/// `keyword_ids` the items matching the keywords. Each item scores its
/// similarity; keyword matches score `max(similarity, min_similarity) +
/// KEYWORD_BONUS`, so they are always included but cannot outrank clearly
/// stronger meaning matches. Items that do not match the keywords must reach
/// [`similarity_floor`]. Ties are broken by the newest (largest) id.
pub fn hybrid_rank(
    similarities: &[(i64, f32)],
    keyword_ids: &[i64],
    min_similarity: f32,
    limit: usize,
) -> Vec<i64> {
    use std::collections::{HashMap, HashSet};

    let mut best: HashMap<i64, f32> = HashMap::new();
    for &(id, similarity) in similarities {
        let entry = best.entry(id).or_insert(f32::NEG_INFINITY);
        *entry = entry.max(similarity);
    }
    let keyword: HashSet<i64> = keyword_ids.iter().copied().collect();
    let floor = similarity_floor(similarities, min_similarity);

    let mut scored: Vec<(i64, f32)> = best
        .iter()
        .filter(|(id, similarity)| **similarity >= floor && !keyword.contains(id))
        .map(|(id, similarity)| (*id, *similarity))
        .collect();
    scored.extend(keyword.iter().map(|id| {
        let similarity = best.get(id).copied().unwrap_or(min_similarity);
        (*id, similarity.max(min_similarity) + KEYWORD_BONUS)
    }));
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

    #[test]
    fn hybrid_strong_meaning_match_outranks_weak_match_with_keywords() {
        // The "AMP with Carter Schultz" case: item 2 mentions the words but is
        // only loosely related; item 1 is clearly about the topic.
        let ranked = hybrid_rank(&[(1, 0.45), (2, 0.22)], &[2], MIN, 10);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_keyword_match_breaks_near_ties() {
        let ranked = hybrid_rank(&[(1, 0.40), (2, 0.35)], &[2], MIN, 10);
        assert_eq!(ranked, vec![2, 1]);
    }

    #[test]
    fn hybrid_keeps_keyword_matches_below_threshold_but_drops_unrelated_items() {
        let ranked = hybrid_rank(&[(1, 0.10), (2, 0.05), (3, 0.35)], &[2], MIN, 10);
        assert_eq!(ranked, vec![3, 2]);
    }

    #[test]
    fn hybrid_includes_keyword_matches_without_embeddings() {
        let ranked = hybrid_rank(&[(1, 0.50), (2, 0.31)], &[9], MIN, 10);
        assert_eq!(ranked, vec![1, 2, 9]);
    }

    #[test]
    fn similarity_floor_is_relative_to_the_best_match() {
        assert_eq!(similarity_floor(&[], MIN), MIN);
        // Long queries score high overall: 60% of the best match.
        assert!((similarity_floor(&[(1, 0.55), (2, 0.3)], MIN) - 0.33).abs() < 1e-6);
        // Short queries score low overall: the absolute minimum wins.
        assert_eq!(similarity_floor(&[(1, 0.25)], MIN), MIN);
    }

    #[test]
    fn hybrid_drops_semantic_matches_far_below_the_best_one() {
        // Benchmark: 40% of random articles score >= 0.2 for title-length
        // queries, whose real matches score around 0.55.
        let ranked = hybrid_rank(&[(1, 0.60), (2, 0.40), (3, 0.30)], &[], MIN, 10);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_keeps_keyword_matches_below_the_relative_floor() {
        let ranked = hybrid_rank(&[(1, 0.60), (2, 0.25)], &[2], MIN, 10);
        assert_eq!(ranked, vec![1, 2]);
    }

    #[test]
    fn hybrid_respects_limit_and_breaks_ties_by_newest_id() {
        let ranked = hybrid_rank(&[(1, 0.3), (5, 0.3), (3, 0.3)], &[], MIN, 2);
        assert_eq!(ranked, vec![5, 3]);
        assert!(hybrid_rank(&[], &[], MIN, 10).is_empty());
    }

    quickcheck::quickcheck! {
        fn prop_hybrid_output_is_unique_and_complete(scores: Vec<(i64, i8)>, keyword: Vec<i64>, limit: u8) -> bool {
            use std::collections::HashSet;
            let scores: Vec<(i64, f32)> = scores
                .into_iter()
                .map(|(id, s)| (id, f32::from(s) / 128.0))
                .collect();
            let ranked = hybrid_rank(&scores, &keyword, MIN, usize::from(limit));
            let floor = similarity_floor(&scores, MIN);
            let mut eligible: HashSet<i64> = keyword.iter().copied().collect();
            eligible.extend(scores.iter().filter(|(_, s)| *s >= floor).map(|(id, _)| *id));
            let unique: HashSet<_> = ranked.iter().collect();
            unique.len() == ranked.len()
                && ranked.iter().all(|id| eligible.contains(id))
                && ranked.len() == eligible.len().min(usize::from(limit))
        }


        fn prop_normalize_yields_unit_length(v: Vec<i16>) -> bool {
            let v: Vec<f32> = v.into_iter().map(f32::from).collect();
            match normalize(&v) {
                Some(n) => (n.iter().map(|x| x * x).sum::<f32>() - 1.0).abs() < 1e-3,
                None => v.iter().all(|x| *x == 0.0),
            }
        }

        fn prop_top_k_is_sorted_and_bounded(scores: Vec<i16>, k: u8) -> bool {
            let vectors: Vec<[f32; 1]> = scores.iter().map(|s| [f32::from(*s)]).collect();
            let result = top_k(&[1.0], vectors.iter().enumerate().map(|(i, v)| (i as i64, &v[..])), usize::from(k));
            result.len() == scores.len().min(usize::from(k))
                && result.windows(2).all(|w| w[0].1 >= w[1].1)
        }
    }
}
