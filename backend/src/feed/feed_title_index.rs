use std::{future::Future, sync::LazyLock};

use anyhow::{Context, Result};
use sqlx::PgPool;

const MIN_WORD_LENGTH: i32 = 2;
static STOP_WORDS: LazyLock<Vec<String>> = LazyLock::new(|| {
    [
        // Norwegian
        "a", "alle", "at", "av", "bare", "ble", "bli", "blir", "da", "de", "deg", "dei", "dem",
        "den", "denne", "der", "dere", "deres", "det", "dette", "din", "disse", "då", "du", "eg",
        "ein", "eit", "eller", "en", "er", "et", "etter", "for", "fordi", "fra", "før", "få",
        "får", "fikk", "ha", "hadde", "han", "hans", "har", "hatt", "hele", "henne", "hennes",
        "her", "hos", "hun", "andre", "flere", "fortsatt", "går", "gir", "gjør", "hva", "hvem",
        "hvilke", "hvilken", "hvis", "hvor", "hvordan", "hvorfor", "i", "ikke", "ingen", "inn",
        "jeg", "kan", "kom", "kunne", "man", "me", "med", "meg", "mellom", "men", "mens", "mer",
        "mi", "min", "mine", "mitt", "mot", "må", "ned", "noe", "noen", "nok", "nå", "når", "og",
        "også", "nye", "om", "opp", "oss", "over", "på", "samme", "seg", "selv", "si", "siden",
        "sier", "sin", "sine", "sitt", "skal", "slik", "som", "så", "til", "under", "uten", "ut",
        "var", "ved", "vi", "vil", "ville", "vår", "våre", "vårt", "være", "å", // English
        "about", "above", "after", "again", "against", "all", "also", "am", "an", "and", "any",
        "are", "as", "at", "be", "because", "been", "before", "being", "below", "between", "both",
        "but", "by", "can", "did", "do", "does", "doing", "down", "during", "each", "few", "from",
        "further", "get", "had", "has", "have", "having", "he", "her", "here", "him", "his", "how",
        "if", "in", "into", "is", "it", "its", "itself", "just", "like", "make", "me", "more",
        "most", "my", "new", "no", "not", "now", "of", "off", "on", "once", "one", "only", "or",
        "other", "our", "out", "over", "own", "s", "said", "same", "say", "says", "she", "should",
        "so", "some", "such", "t", "than", "that", "the", "their", "them", "then", "there",
        "these", "they", "this", "those", "through", "to", "too", "under", "until", "up", "us",
        "very", "was", "we", "were", "what", "when", "where", "which", "while", "who", "whom",
        "why", "will", "with", "you", "your",
    ]
    .into_iter()
    .map(String::from)
    .collect()
});

#[derive(Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct FeedTitleIndexItem {
    pub feed_src_id: i64,
    #[serde(rename = "occurences")]
    pub occurrences: u64,
}

#[derive(Debug, PartialEq, serde::Serialize, utoipa::ToSchema)]
pub struct FeedTitleIndexEntry {
    pub word: String,
    #[serde(rename = "total_occurences")]
    pub total_occurrences: u64,
    /// Number of distinct feed item titles containing this word at least once.
    pub document_frequency: u64,
    /// TF-IDF score: `total_occurrences * ln(total_documents / document_frequency)`.
    /// Words that appear in nearly every title score close to `0.0` (uninformative);
    /// words concentrated in a small subset of titles score higher.
    pub tf_idf: f64,
    pub items: Vec<FeedTitleIndexItem>,
}

struct FeedTitleIndexRow {
    word: String,
    feed_src_id: i64,
    occurrences: i64,
    total_occurrences: i64,
    document_frequency: i64,
    total_documents: i64,
}

pub trait FeedTitleIndexRepository {
    fn read_feed_title_index(
        &self,
    ) -> impl Future<Output = Result<Vec<FeedTitleIndexEntry>>> + Send;

    fn read_recent_feed_title_index(
        &self,
    ) -> impl Future<Output = Result<Vec<FeedTitleIndexEntry>>> + Send;
}

impl FeedTitleIndexRepository for PgPool {
    async fn read_feed_title_index(&self) -> Result<Vec<FeedTitleIndexEntry>> {
        let rows = read_feed_title_index_rows(self).await?;
        group_rows(rows)
    }

    async fn read_recent_feed_title_index(&self) -> Result<Vec<FeedTitleIndexEntry>> {
        let rows = read_recent_feed_title_index_rows(self).await?;
        group_rows(rows)
    }
}

pub async fn read_feed_title_index(pool: &PgPool) -> Result<Vec<FeedTitleIndexEntry>> {
    pool.read_feed_title_index().await
}

pub async fn read_recent_feed_title_index(pool: &PgPool) -> Result<Vec<FeedTitleIndexEntry>> {
    pool.read_recent_feed_title_index().await
}

async fn read_feed_title_index_rows(pool: &PgPool) -> Result<Vec<FeedTitleIndexRow>> {
    sqlx::query_as!(
        FeedTitleIndexRow,
        r#"
        WITH words AS (
            SELECT
                id AS item_id,
                feed_id,
                lower(regexp_split_to_table(title, '[^a-zA-ZæøåÆØÅ]+')) AS word
            FROM feed_items
        ),
        filtered_words AS (
            SELECT item_id, feed_id, word
            FROM words
            WHERE length(word) >= $1 AND word != ALL($2::TEXT[])
        ),
        counted_words AS (
            SELECT
                feed_id,
                word,
                COUNT(*)::BIGINT AS occurrences
            FROM filtered_words
            GROUP BY feed_id, word
        ),
        totals AS (
            SELECT
                word,
                SUM(occurrences)::BIGINT AS total_occurrences
            FROM counted_words
            GROUP BY word
        ),
        document_frequencies AS (
            SELECT
                word,
                COUNT(DISTINCT item_id)::BIGINT AS document_frequency
            FROM filtered_words
            GROUP BY word
        ),
        corpus_size AS (
            SELECT COUNT(*)::BIGINT AS total_documents FROM feed_items
        )
        SELECT
            cw.word AS "word!",
            cw.feed_id AS "feed_src_id!",
            cw.occurrences AS "occurrences!",
            t.total_occurrences AS "total_occurrences!",
            df.document_frequency AS "document_frequency!",
            corpus_size.total_documents AS "total_documents!"
        FROM counted_words cw
        JOIN totals t USING (word)
        JOIN document_frequencies df USING (word)
        CROSS JOIN corpus_size
        ORDER BY cw.word ASC, cw.occurrences DESC, cw.feed_id ASC
        "#,
        MIN_WORD_LENGTH,
        STOP_WORDS.as_slice(),
    )
    .fetch_all(pool)
    .await
    .context("failed to read feed title index")
}

async fn read_recent_feed_title_index_rows(pool: &PgPool) -> Result<Vec<FeedTitleIndexRow>> {
    sqlx::query_as!(
        FeedTitleIndexRow,
        r#"
        WITH words AS (
            SELECT
                id AS item_id,
                feed_id,
                lower(regexp_split_to_table(title, '[^a-zA-ZæøåÆØÅ]+')) AS word
            FROM feed_items
            WHERE inserted_at >= NOW() - INTERVAL '24 hours'
        ),
        filtered_words AS (
            SELECT item_id, feed_id, word
            FROM words
            WHERE length(word) >= $1 AND word != ALL($2::TEXT[])
        ),
        counted_words AS (
            SELECT
                feed_id,
                word,
                COUNT(*)::BIGINT AS occurrences
            FROM filtered_words
            GROUP BY feed_id, word
        ),
        totals AS (
            SELECT
                word,
                SUM(occurrences)::BIGINT AS total_occurrences
            FROM counted_words
            GROUP BY word
        ),
        document_frequencies AS (
            SELECT
                word,
                COUNT(DISTINCT item_id)::BIGINT AS document_frequency
            FROM filtered_words
            GROUP BY word
        ),
        corpus_size AS (
            SELECT COUNT(*)::BIGINT AS total_documents
            FROM feed_items
            WHERE inserted_at >= NOW() - INTERVAL '24 hours'
        )
        SELECT
            cw.word AS "word!",
            cw.feed_id AS "feed_src_id!",
            cw.occurrences AS "occurrences!",
            t.total_occurrences AS "total_occurrences!",
            df.document_frequency AS "document_frequency!",
            corpus_size.total_documents AS "total_documents!"
        FROM counted_words cw
        JOIN totals t USING (word)
        JOIN document_frequencies df USING (word)
        CROSS JOIN corpus_size
        ORDER BY cw.word ASC, cw.occurrences DESC, cw.feed_id ASC
        "#,
        MIN_WORD_LENGTH,
        STOP_WORDS.as_slice(),
    )
    .fetch_all(pool)
    .await
    .context("failed to read recent feed title index")
}

fn count_to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).context("title index count was negative")
}

/// Computes the TF-IDF score for a word: `tf * ln(N / df)`.
///
/// * `tf` (`total_occurrences`) is how many times the word occurs across the
///   whole corpus.
/// * `N` (`total_documents`) is the number of feed item titles in the corpus.
/// * `df` (`document_frequency`) is how many distinct titles contain the word
///   at least once.
///
/// A word present in every title has `df == N`, so `ln(N / df) == 0`: it
/// carries no discriminative value regardless of how often it repeats. Words
/// concentrated in a small subset of titles score higher.
fn compute_tf_idf(total_occurrences: u64, document_frequency: u64, total_documents: u64) -> f64 {
    if document_frequency == 0 || total_documents == 0 {
        return 0.0;
    }
    let idf = (total_documents as f64 / document_frequency as f64).ln();
    total_occurrences as f64 * idf
}

fn group_rows(rows: Vec<FeedTitleIndexRow>) -> Result<Vec<FeedTitleIndexEntry>> {
    let mut entries: Vec<FeedTitleIndexEntry> = Vec::new();
    for row in rows {
        let occurrences = count_to_u64(row.occurrences)?;
        if let Some(entry) = entries.last_mut()
            && entry.word == row.word
        {
            entry.items.push(FeedTitleIndexItem {
                feed_src_id: row.feed_src_id,
                occurrences,
            });
            continue;
        }

        let total_occurrences = count_to_u64(row.total_occurrences)?;
        let document_frequency = count_to_u64(row.document_frequency)?;
        let total_documents = count_to_u64(row.total_documents)?;
        let tf_idf = compute_tf_idf(total_occurrences, document_frequency, total_documents);
        entries.push(FeedTitleIndexEntry {
            word: row.word,
            total_occurrences,
            document_frequency,
            tf_idf,
            items: vec![FeedTitleIndexItem {
                feed_src_id: row.feed_src_id,
                occurrences,
            }],
        });
    }

    entries.sort_by(|a, b| {
        b.tf_idf
            .partial_cmp(&a.tf_idf)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.total_occurrences.cmp(&a.total_occurrences))
            .then_with(|| a.word.cmp(&b.word))
    });

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::feed::{feed_item::insert_feed_item, feed_subscription::upsert_feed_by_url};

    fn find_entry<'a>(
        index: &'a [FeedTitleIndexEntry],
        word: &str,
    ) -> Option<&'a FeedTitleIndexEntry> {
        index.iter().find(|entry| entry.word == word)
    }

    #[sqlx::test]
    async fn test_feed_title_index(pool: sqlx::PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed.id, "ext-1", "Title", "https://example.com")
            .await
            .unwrap();

        let index = read_feed_title_index(&pool).await.unwrap();
        let title = find_entry(&index, "title").unwrap();

        assert_eq!(title.total_occurrences, 1);
        assert_eq!(title.items.len(), 1);
        assert_eq!(title.items[0].feed_src_id, feed.id);
    }

    #[sqlx::test]
    async fn test_feed_title_index_multiple_feeds(pool: sqlx::PgPool) {
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

        let index = read_feed_title_index(&pool).await.unwrap();
        let title = find_entry(&index, "title").unwrap();

        assert_eq!(title.total_occurrences, 2);
        assert_eq!(title.items.len(), 2);
        assert!(title.items.iter().any(|item| item.feed_src_id == feed1.id));
        assert!(title.items.iter().any(|item| item.feed_src_id == feed2.id));
    }

    #[sqlx::test]
    async fn test_feed_title_index_sorted_by_tf_idf(pool: sqlx::PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        // "alpha" appears in every title in the corpus: high raw frequency,
        // but zero discriminative value once every document contains it.
        insert_feed_item(
            &pool,
            feed.id,
            "ext-1",
            "Alpha Alpha",
            "https://example.com",
        )
        .await
        .unwrap();
        insert_feed_item(&pool, feed.id, "ext-2", "Alpha", "https://example.com")
            .await
            .unwrap();
        // "beta" only shows up in this one title out of three.
        insert_feed_item(
            &pool,
            feed.id,
            "ext-3",
            "Alpha Beta Beta",
            "https://example.com",
        )
        .await
        .unwrap();

        let index = read_feed_title_index(&pool).await.unwrap();

        let alpha = find_entry(&index, "alpha").unwrap();
        let beta = find_entry(&index, "beta").unwrap();

        assert_eq!(alpha.total_occurrences, 4);
        assert_eq!(alpha.document_frequency, 3);
        assert_eq!(alpha.tf_idf, 0.0); // present in every title: no signal

        assert_eq!(beta.total_occurrences, 2);
        assert_eq!(beta.document_frequency, 1);
        assert!(beta.tf_idf > 0.0);

        // Despite fewer raw occurrences, "beta" is more distinctive than the
        // ubiquitous "alpha" and should be ranked first.
        assert_eq!(index[0].word, "beta");
    }

    #[sqlx::test]
    async fn test_feed_title_index_filters_stop_words(pool: sqlx::PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(
            &pool,
            feed.id,
            "ext-1",
            "The quick and brown fox på norsk og engelsk med det som er vanlig",
            "https://example.com",
        )
        .await
        .unwrap();

        let index = read_feed_title_index(&pool).await.unwrap();

        assert!(find_entry(&index, "the").is_none());
        assert!(find_entry(&index, "and").is_none());
        assert!(find_entry(&index, "på").is_none());
        assert!(find_entry(&index, "og").is_none());
        assert!(find_entry(&index, "med").is_none());
        assert!(find_entry(&index, "det").is_none());
        assert!(find_entry(&index, "som").is_none());
        assert!(find_entry(&index, "er").is_none());
        assert!(find_entry(&index, "quick").is_some());
        assert!(find_entry(&index, "norsk").is_some());
    }

    #[sqlx::test]
    async fn test_feed_title_index_normalizes_words_with_postgresql(pool: sqlx::PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(
            &pool,
            feed.id,
            "ext-1",
            "Rust, rust! state of the art --rust--",
            "https://example.com",
        )
        .await
        .unwrap();

        let index = read_feed_title_index(&pool).await.unwrap();
        let rust = find_entry(&index, "rust").unwrap();

        assert_eq!(rust.total_occurrences, 3);
        assert!(find_entry(&index, "state").is_some());
    }

    #[sqlx::test]
    async fn test_recent_feed_title_index(pool: sqlx::PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        let old_item = insert_feed_item(
            &pool,
            feed.id,
            "old-1",
            "Archived Technology",
            "https://example.com/old",
        )
        .await
        .unwrap();
        sqlx::query("UPDATE feed_items SET inserted_at = NOW() - INTERVAL '2 days' WHERE id = $1")
            .bind(old_item.id)
            .execute(&pool)
            .await
            .unwrap();

        insert_feed_item(
            &pool,
            feed.id,
            "today-1",
            "Breaking Technology News",
            "https://example.com/1",
        )
        .await
        .unwrap();

        let index = read_recent_feed_title_index(&pool).await.unwrap();

        assert!(find_entry(&index, "archived").is_none());
        let technology = find_entry(&index, "technology").unwrap();
        assert_eq!(technology.total_occurrences, 1);
    }

    #[sqlx::test]
    async fn test_recent_feed_title_index_empty(pool: sqlx::PgPool) {
        let index = read_recent_feed_title_index(&pool).await.unwrap();
        assert!(index.is_empty());
    }

    #[test]
    fn test_feed_title_index_serializes_existing_api_field_names() {
        let json = serde_json::to_value(FeedTitleIndexEntry {
            word: "rust".to_string(),
            total_occurrences: 2,
            document_frequency: 1,
            tf_idf: 2.1972,
            items: vec![FeedTitleIndexItem {
                feed_src_id: 1,
                occurrences: 2,
            }],
        })
        .unwrap();

        assert_eq!(json["total_occurences"], 2);
        assert_eq!(json["items"][0]["occurences"], 2);
        assert!(json.get("total_occurrences").is_none());
        assert!(json["items"][0].get("occurrences").is_none());

        // New fields are not part of the misspelling grandfathered in above
        // and should serialize under their correctly-spelled names.
        assert_eq!(json["document_frequency"], 1);
        assert_eq!(json["tf_idf"], 2.1972);
    }

    #[test]
    fn test_compute_tf_idf_is_zero_for_a_word_in_every_document() {
        assert_eq!(compute_tf_idf(10, 5, 5), 0.0);
    }

    #[test]
    fn test_compute_tf_idf_rewards_words_concentrated_in_fewer_documents() {
        let ubiquitous = compute_tf_idf(4, 3, 3);
        let rare = compute_tf_idf(2, 1, 3);

        assert_eq!(ubiquitous, 0.0);
        assert!(rare > ubiquitous);
    }

    #[test]
    fn test_compute_tf_idf_scales_linearly_with_term_frequency() {
        let once = compute_tf_idf(1, 1, 10);
        let five_times = compute_tf_idf(5, 1, 10);

        assert_eq!(five_times, once * 5.0);
    }
}
