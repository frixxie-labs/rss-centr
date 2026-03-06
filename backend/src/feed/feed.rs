use anyhow::{Context, Result};
use feed_rs::model::Feed;
use feed_rs::parser;
use reqwest::{StatusCode, header};

pub enum FetchFeedOutcome {
    NotModified {
        etag: Option<String>,
        last_modified: Option<String>,
    },
    Fetched {
        feed: Box<Feed>,
        etag: Option<String>,
        last_modified: Option<String>,
    },
}

/// Fetches and parses an RSS feed from the given URL.
///
/// Uses the provided `reqwest::Client` to download the feed bytes,
/// then parses them into a `feed_rs::model::Feed`.
pub async fn fetch_feed(client: &reqwest::Client, url: &str) -> Result<Feed> {
    let outcome = fetch_feed_with_cache(client, url, None, None).await?;
    match outcome {
        FetchFeedOutcome::Fetched { feed, .. } => Ok(*feed),
        FetchFeedOutcome::NotModified { .. } => {
            anyhow::bail!("feed returned not-modified without cache validators")
        }
    }
}

pub async fn fetch_feed_with_cache(
    client: &reqwest::Client,
    url: &str,
    etag: Option<&str>,
    last_modified: Option<&str>,
) -> Result<FetchFeedOutcome> {
    let mut request = client.get(url);
    if let Some(etag) = etag {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    if let Some(last_modified) = last_modified {
        request = request.header(header::IF_MODIFIED_SINCE, last_modified);
    }

    let response = request
        .send()
        .await
        .with_context(|| format!("Failed to fetch feed from {url}"))?;

    let response_etag = header_value_to_string(response.headers().get(header::ETAG));
    let response_last_modified =
        header_value_to_string(response.headers().get(header::LAST_MODIFIED));

    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(FetchFeedOutcome::NotModified {
            etag: response_etag,
            last_modified: response_last_modified,
        });
    }

    let response = response
        .error_for_status()
        .with_context(|| format!("Non-success status fetching feed from {url}"))?;

    let bytes = response
        .bytes()
        .await
        .with_context(|| format!("Failed to read response body from {url}"))?;

    let feed =
        parser::parse(&bytes[..]).with_context(|| format!("Failed to parse feed from {url}"))?;

    Ok(FetchFeedOutcome::Fetched {
        feed: Box::new(feed),
        etag: response_etag,
        last_modified: response_last_modified,
    })
}

fn header_value_to_string(value: Option<&header::HeaderValue>) -> Option<String> {
    value.and_then(|v| v.to_str().ok()).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub const FEED_URLS: &[&str] = &[
        "https://www.nrk.no/nyheter/siste.rss",
        "https://rss.kode24.no/",
        "https://www.adressa.no/rss",
        "https://www.tek.no/api/rss/rss2/medium/collections",
    ];

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
