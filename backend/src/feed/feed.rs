use anyhow::{Context, Result};
use feed_rs::model::Feed;
use feed_rs::parser;

pub const FEED_URLS: &[&str] = &[
    "https://www.nrk.no/nyheter/siste.rss",
    "https://rss.kode24.no/",
    "https://www.adressa.no/rss",
    "https://www.tek.no/api/rss/rss2/medium/collections",
];

/// Fetches and parses an RSS feed from the given URL.
///
/// Uses the provided `reqwest::Client` to download the feed bytes,
/// then parses them into a `feed_rs::model::Feed`.
pub async fn fetch_feed(client: &reqwest::Client, url: &str) -> Result<Feed> {
    let response = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("Failed to fetch feed from {url}"))?;

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read response body from {url}"))?;

    let feed =
        parser::parse(&bytes[..]).with_context(|| format!("Failed to parse feed from {url}"))?;

    Ok(feed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_fetch_all_feeds() {
        let client = reqwest::Client::new();
        for url in FEED_URLS {
            let feed = fetch_feed(&client, url)
                .await
                .unwrap_or_else(|e| panic!("failed to fetch {url}: {e}"));

            assert!(feed.title.is_some(), "feed from {url} should have a title");
            assert!(
                !feed.entries.is_empty(),
                "feed from {url} should have entries"
            );
        }
    }
}
