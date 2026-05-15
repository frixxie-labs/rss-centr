use std::future::Future;

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};

const MIN_WORD_LENGTH: i32 = 2;
const STOP_WORDS: &[&str] = &[
    // Norwegian
    "a", "alle", "at", "av", "bare", "ble", "bli", "blir", "da", "de", "deg", "dei", "dem", "den",
    "denne", "der", "dere", "deres", "det", "dette", "din", "disse", "då", "du", "eg", "ein",
    "eit", "eller", "en", "er", "et", "etter", "for", "fordi", "fra", "før", "få", "får", "fikk",
    "ha", "hadde", "han", "hans", "har", "hatt", "hele", "henne", "hennes", "her", "hos", "hun",
    "andre", "flere", "fortsatt", "går", "gir", "gjør", "hva", "hvem", "hvilke", "hvilken", "hvis",
    "hvor", "hvordan", "hvorfor", "i", "ikke", "ingen", "inn", "jeg", "kan", "kom", "kunne", "man",
    "me", "med", "meg", "mellom", "men", "mens", "mer", "mi", "min", "mine", "mitt", "mot", "må",
    "ned", "noe", "noen", "nok", "nå", "når", "og", "også", "nye", "om", "opp", "oss", "over",
    "på", "samme", "seg", "selv", "si", "siden", "sier", "sin", "sine", "sitt", "skal", "slik",
    "som", "så", "til", "under", "uten", "ut", "var", "ved", "vi", "vil", "ville", "vår", "våre",
    "vårt", "være", "å", // English
    "about", "above", "after", "again", "against", "all", "also", "am", "an", "and", "any", "are",
    "as", "at", "be", "because", "been", "before", "being", "below", "between", "both", "but",
    "by", "can", "did", "do", "does", "doing", "down", "during", "each", "few", "from", "further",
    "get", "had", "has", "have", "having", "he", "her", "here", "him", "his", "how", "if", "in",
    "into", "is", "it", "its", "itself", "just", "like", "make", "me", "more", "most", "my", "new",
    "no", "not", "now", "of", "off", "on", "once", "one", "only", "or", "other", "our", "out",
    "over", "own", "s", "said", "same", "say", "says", "she", "should", "so", "some", "such", "t",
    "than", "that", "the", "their", "them", "then", "there", "these", "they", "this", "those",
    "through", "to", "too", "under", "until", "up", "us", "very", "was", "we", "were", "what",
    "when", "where", "which", "while", "who", "whom", "why", "will", "with", "you", "your",
];

#[derive(Debug, Ord, PartialEq, PartialOrd, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct FeedTitleIndexItem {
    pub feed_src_id: i64,
    pub occurences: u64,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize, utoipa::ToSchema)]
pub struct FeedTitleIndexEntry {
    pub word: String,
    pub total_occurences: u64,
    pub items: Vec<FeedTitleIndexItem>,
}

struct FeedTitleIndexRow {
    word: String,
    feed_src_id: i64,
    occurences: u64,
    total_occurences: u64,
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
        read_feed_title_index_filtered(self, false).await
    }

    async fn read_recent_feed_title_index(&self) -> Result<Vec<FeedTitleIndexEntry>> {
        read_feed_title_index_filtered(self, true).await
    }
}

pub async fn read_feed_title_index(pool: &PgPool) -> Result<Vec<FeedTitleIndexEntry>> {
    pool.read_feed_title_index().await
}

pub async fn read_recent_feed_title_index(pool: &PgPool) -> Result<Vec<FeedTitleIndexEntry>> {
    pool.read_recent_feed_title_index().await
}

async fn read_feed_title_index_filtered(
    pool: &PgPool,
    recent_only: bool,
) -> Result<Vec<FeedTitleIndexEntry>> {
    let rows = sqlx::query(
        r#"
        WITH words AS (
            SELECT
                feed_id,
                lower(regexp_split_to_table(title, '[^a-zA-ZæøåÆØÅ]+')) AS word
            FROM feed_items
            WHERE $1::BOOLEAN IS FALSE OR inserted_at >= NOW() - INTERVAL '24 hours'
        ),
        counted_words AS (
            SELECT
                feed_id,
                word,
                COUNT(*)::BIGINT AS occurences
            FROM words
            WHERE length(word) >= $2 AND word != ALL($3::TEXT[])
            GROUP BY feed_id, word
        ),
        totals AS (
            SELECT
                word,
                SUM(occurences)::BIGINT AS total_occurences
            FROM counted_words
            GROUP BY word
        )
        SELECT
            cw.word,
            cw.feed_id AS feed_src_id,
            cw.occurences,
            t.total_occurences
        FROM counted_words cw
        JOIN totals t USING (word)
        ORDER BY t.total_occurences DESC, cw.word ASC, cw.occurences DESC, cw.feed_id ASC
        "#,
    )
    .bind(recent_only)
    .bind(MIN_WORD_LENGTH)
    .bind(STOP_WORDS)
    .fetch_all(pool)
    .await
    .context("failed to read feed title index")?;

    let rows = rows
        .into_iter()
        .map(|row| -> Result<FeedTitleIndexRow> {
            Ok(FeedTitleIndexRow {
                word: row.try_get("word")?,
                feed_src_id: row.try_get("feed_src_id")?,
                occurences: count_to_u64(row.try_get("occurences")?)?,
                total_occurences: count_to_u64(row.try_get("total_occurences")?)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(group_rows(rows))
}

fn count_to_u64(value: i64) -> Result<u64> {
    u64::try_from(value).context("title index count was negative")
}

fn group_rows(rows: Vec<FeedTitleIndexRow>) -> Vec<FeedTitleIndexEntry> {
    let mut entries: Vec<FeedTitleIndexEntry> = Vec::new();
    for row in rows {
        if let Some(entry) = entries.last_mut()
            && entry.word == row.word
        {
            entry.items.push(FeedTitleIndexItem {
                feed_src_id: row.feed_src_id,
                occurences: row.occurences,
            });
            continue;
        }

        entries.push(FeedTitleIndexEntry {
            word: row.word,
            total_occurences: row.total_occurences,
            items: vec![FeedTitleIndexItem {
                feed_src_id: row.feed_src_id,
                occurences: row.occurences,
            }],
        });
    }
    entries
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

        assert_eq!(title.total_occurences, 1);
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

        assert_eq!(title.total_occurences, 2);
        assert_eq!(title.items.len(), 2);
        assert!(title.items.iter().any(|item| item.feed_src_id == feed1.id));
        assert!(title.items.iter().any(|item| item.feed_src_id == feed2.id));
    }

    #[sqlx::test]
    async fn test_feed_title_index_sorted_by_total_occurences(pool: sqlx::PgPool) {
        let feed = upsert_feed_by_url(&pool, "https://example.com/feed.xml")
            .await
            .unwrap();

        insert_feed_item(&pool, feed.id, "ext-1", "Title One", "https://example.com")
            .await
            .unwrap();
        insert_feed_item(&pool, feed.id, "ext-2", "Title Two", "https://example.com")
            .await
            .unwrap();
        insert_feed_item(&pool, feed.id, "ext-3", "Title", "https://example.com")
            .await
            .unwrap();

        let index = read_feed_title_index(&pool).await.unwrap();

        assert_eq!(index[0].word, "title");
        assert_eq!(index[0].total_occurences, 3);
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

        assert_eq!(rust.total_occurences, 3);
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
        assert_eq!(technology.total_occurences, 1);
    }

    #[sqlx::test]
    async fn test_recent_feed_title_index_empty(pool: sqlx::PgPool) {
        let index = read_recent_feed_title_index(&pool).await.unwrap();
        assert!(index.is_empty());
    }
}
