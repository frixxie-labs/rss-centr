use std::future::Future;

use anyhow::{Context, Result};
use sqlx::{PgPool, Row};

/// Common English stop words that carry little semantic meaning in titles.
const ENGLISH_STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "it", "in", "on", "at", "to", "of", "and", "or", "but", "not", "no",
    "for", "by", "with", "from", "up", "as", "do", "if", "be", "so", "we", "he", "she", "me", "my",
    "am", "are", "was", "has", "had", "its", "you", "your", "they", "them", "our", "us", "this",
    "that", "will", "can", "how", "what", "when", "who", "all", "been", "have", "were", "which",
    "their", "there", "about", "would", "could", "should", "just", "than", "then", "also", "into",
    "only", "very", "some", "more", "over", "such", "after", "does",
];

/// Common Norwegian (Bokmal/Nynorsk) stop words.
const NORWEGIAN_STOP_WORDS: &[&str] = &[
    "og",
    "i",
    "jeg",
    "det",
    "at",
    "en",
    "et",
    "den",
    "til",
    "er",
    "som",
    "på",
    "de",
    "med",
    "han",
    "av",
    "ikke",
    "ikkje",
    "der",
    "så",
    "var",
    "meg",
    "seg",
    "men",
    "ett",
    "har",
    "om",
    "vi",
    "min",
    "mitt",
    "ha",
    "hadde",
    "hun",
    "nå",
    "over",
    "da",
    "ved",
    "fra",
    "du",
    "ut",
    "sin",
    "dem",
    "oss",
    "opp",
    "man",
    "kan",
    "hans",
    "hvor",
    "eller",
    "hva",
    "skal",
    "selv",
    "sjøl",
    "her",
    "alle",
    "vil",
    "bli",
    "ble",
    "blitt",
    "kunne",
    "inn",
    "når",
    "være",
    "kom",
    "noen",
    "noe",
    "ville",
    "dere",
    "deres",
    "kun",
    "ja",
    "etter",
    "ned",
    "skulle",
    "denne",
    "for",
    "deg",
    "si",
    "sine",
    "sitt",
    "mot",
    "å",
    "meget",
    "hvorfor",
    "dette",
    "disse",
    "uten",
    "hvordan",
    "ingen",
    "din",
    "ditt",
    "blir",
    "samme",
    "hvilken",
    "hvilke",
    "sånn",
    "inni",
    "mellom",
    "vår",
    "hver",
    "hvem",
    "vors",
    "hvis",
    "både",
    "bare",
    "enn",
    "fordi",
    "før",
    "mange",
    "også",
    "slik",
    "vært",
    "begge",
    "siden",
    "dykk",
    "dykkar",
    "dei",
    "deira",
    "deires",
    "deim",
    "di",
    "då",
    "eg",
    "ein",
    "eit",
    "eitt",
    "elles",
    "honom",
    "hjå",
    "ho",
    "hoe",
    "henne",
    "hennar",
    "hennes",
    "hoss",
    "hossen",
    "ingi",
    "inkje",
    "korleis",
    "korso",
    "kva",
    "kvar",
    "kvarhelst",
    "kven",
    "kvi",
    "kvifor",
    "me",
    "medan",
    "mi",
    "mine",
    "mykje",
    "no",
    "nokon",
    "noka",
    "nokor",
    "noko",
    "nokre",
    "sia",
    "sidan",
    "so",
    "somt",
    "somme",
    "um",
    "upp",
    "vere",
    "vore",
    "verte",
    "vort",
    "varte",
    "vart",
    "tilbake",
    "igjen",
    "enda",
    "allerede",
    "alltid",
    "ofte",
    "bort",
    "bra",
    "deles",
    "dessuten",
    "deretter",
    "derfor",
    "dermed",
    "derimot",
    "derfra",
    "derinne",
    "deroppe",
    "derute",
    "derved",
    "etterpå",
    "frem",
    "fremfor",
    "ganske",
    "gjennom",
    "gjorde",
    "gjort",
    "gjør",
    "gjøre",
    "gjerne",
    "hele",
    "heller",
    "helt",
    "hittil",
    "hverandre",
    "imidlertid",
    "innen",
    "langs",
    "lenge",
    "like",
    "likevel",
    "litt",
    "mer",
    "mest",
    "mindre",
    "mulig",
    "muligens",
    "nemlig",
    "nok",
    "noenlunde",
    "nylig",
    "nær",
    "nærmere",
    "rundt",
    "samtlige",
    "senere",
    "sist",
    "sjelden",
    "snart",
    "stadig",
    "straks",
    "svært",
    "tidligere",
    "tillegg",
    "tilsammen",
    "under",
    "underveis",
    "vanlig",
    "vanligvis",
    "vel",
    "veldig",
    "videre",
    "virkelig",
    "visst",
    "ytterligere",
];

const MIN_WORD_LENGTH: i32 = 2;

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
    let stop_words = stop_words();
    let rows = sqlx::query(
        r#"
        WITH indexed_feeds AS (
            SELECT DISTINCT feed_id
            FROM feed_items
            WHERE ($1::BOOLEAN IS FALSE OR inserted_at >= NOW() - INTERVAL '24 hours')
        ),
        counted_words AS (
            SELECT
                indexed_feeds.feed_id,
                stats.word,
                stats.nentry::BIGINT AS occurences
            FROM indexed_feeds
            CROSS JOIN LATERAL ts_stat(format(
                'SELECT to_tsvector(''simple'', title) FROM feed_items WHERE feed_id = %s AND (%L IS FALSE OR inserted_at >= NOW() - INTERVAL ''24 hours'')',
                indexed_feeds.feed_id,
                $1::BOOLEAN
            )) AS stats
            WHERE length(stats.word) >= $2
              AND NOT (stats.word = ANY($3::TEXT[]))
        )
        SELECT
            word,
            feed_id AS feed_src_id,
            occurences,
            SUM(occurences) OVER (PARTITION BY word)::BIGINT AS total_occurences
        FROM counted_words
        ORDER BY total_occurences DESC, word ASC, occurences DESC, feed_src_id ASC
        "#,
    )
    .bind(recent_only)
    .bind(MIN_WORD_LENGTH)
    .bind(stop_words)
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

fn stop_words() -> Vec<String> {
    ENGLISH_STOP_WORDS
        .iter()
        .chain(NORWEGIAN_STOP_WORDS.iter())
        .map(|word| word.to_string())
        .collect()
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
            "The quick and brown fox på norsk og engelsk",
            "https://example.com",
        )
        .await
        .unwrap();

        let index = read_feed_title_index(&pool).await.unwrap();

        assert!(find_entry(&index, "the").is_none());
        assert!(find_entry(&index, "and").is_none());
        assert!(find_entry(&index, "på").is_none());
        assert!(find_entry(&index, "og").is_none());
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
