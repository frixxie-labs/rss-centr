mod feed;

use anyhow::Result;
use feed::FEED_URLS;

#[tokio::main]
async fn main() -> Result<()> {
    let client = reqwest::Client::new();

    for url in FEED_URLS {
        match feed::fetch_feed(&client, url).await {
            Ok(f) => {
                let title = f
                    .title
                    .as_ref()
                    .map(|t| t.content.as_str())
                    .unwrap_or("(no title)");

                println!("Feed: {title}  ({url})");
                println!("Entries: {}", f.entries.len());

                for entry in &f.entries {
                    let entry_title = entry
                        .title
                        .as_ref()
                        .map(|t| t.content.as_str())
                        .unwrap_or("(no title)");

                    let link = entry
                        .links
                        .first()
                        .map(|l| l.href.as_str())
                        .unwrap_or("(no link)");

                    println!("  - {entry_title}");
                    println!("    {link}");
                }
                println!();
            }
            Err(e) => {
                eprintln!("Error fetching {url}: {e:#}");
            }
        }
    }

    Ok(())
}
