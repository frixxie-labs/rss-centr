use feed_rs::{model::Feed, parser};
use quick_xml::{NsReader, events::Event, name::ResolveResult};

pub(crate) fn parse_feed(bytes: &[u8]) -> anyhow::Result<Feed> {
    match parser::parse(bytes) {
        Ok(feed) => Ok(feed),
        Err(parser::ParseFeedError::ParseError(parser::ParseErrorKind::MissingContent {
            ..
        })) => {
            // feed-rs 3 rejects empty optional Media RSS text elements, such as
            // NRK's <media:title />. Retry without those elements only.
            let normalized = without_empty_media_text(bytes)?;
            Ok(parser::parse(normalized.as_slice())?)
        }
        Err(error) => Err(error.into()),
    }
}

fn without_empty_media_text(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut reader = NsReader::from_reader(bytes);
    let mut output = Vec::with_capacity(bytes.len());
    let mut copied_until = 0;
    let mut empty_start = None;

    loop {
        let start = reader.buffer_position() as usize;
        let (namespace, event) = reader.read_resolved_event()?;
        let is_media = matches!(namespace, ResolveResult::Bound(ns) if ns.as_ref() == "http://search.yahoo.com/mrss/");
        let remove_from = match event {
            Event::Start(element) => {
                empty_start = (is_media
                    && matches!(element.local_name().as_ref(), "title" | "description"))
                .then_some(start);
                None
            }
            Event::Empty(element) => {
                empty_start = None;
                (is_media && matches!(element.local_name().as_ref(), "title" | "description"))
                    .then_some(start)
            }
            Event::End(_) => empty_start.take(),
            Event::Text(text) if text.as_ref().bytes().all(|b| b.is_ascii_whitespace()) => None,
            Event::CData(text) if text.as_ref().bytes().all(|b| b.is_ascii_whitespace()) => None,
            Event::Comment(_) => None,
            Event::Eof => break,
            _ => {
                empty_start = None;
                None
            }
        };
        if let Some(remove_from) = remove_from {
            output.extend_from_slice(&bytes[copied_until..remove_from]);
            copied_until = reader.buffer_position() as usize;
        }
    }
    // Copy original byte spans rather than reserializing XML, preserving its
    // encoding, CDATA, entities, and all non-empty metadata verbatim.
    output.extend_from_slice(&bytes[copied_until..]);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nrk_feed(media_title: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<rss xmlns:media="http://search.yahoo.com/mrss/" version="2.0">
  <channel>
    <title>Siste – Siste nytt – NRK</title>
    <link>https://www.nrk.no/nyheter/</link>
    <description />
    <item>
      <title>Lokførerforbundet: Kontakt med Vy i helgen</title>
      <link>https://www.nrk.no/nyheter/lokforerforbundet-1.18038274</link>
      <description>Streiken fortsetter.</description>
      <guid isPermaLink="false">1.18038274</guid>
      <media:content medium="image" type="image/jpeg" url="https://gfx.nrk.no/image.jpg">
        <media:credit role="photographer" scheme="urn:ebu">Fredrik Varfjell/NTB</media:credit>
        {media_title}
      </media:content>
    </item>
  </channel>
</rss>"#
        )
    }

    #[test]
    fn accepts_nrk_feed_with_empty_media_title() {
        for title in [
            "<media:title />",
            "<media:title></media:title>",
            "<media:title> \n </media:title>",
        ] {
            let feed = parse_feed(nrk_feed(title).as_bytes()).unwrap();
            assert_eq!(feed.entries.len(), 1);
            let entry = &feed.entries[0];
            assert_eq!(entry.id, "1.18038274");
            assert_eq!(
                entry.title.as_ref().unwrap().content,
                "Lokførerforbundet: Kontakt med Vy i helgen"
            );
            assert_eq!(
                entry.media[0].content[0].url.as_ref().unwrap().as_str(),
                "https://gfx.nrk.no/image.jpg"
            );
            assert!(
                entry.media[0]
                    .title
                    .as_ref()
                    .is_none_or(|title| title.content.trim().is_empty())
            );
        }
    }

    #[test]
    fn accepts_nrk_feed_without_media_title() {
        assert!(parse_feed(nrk_feed("").as_bytes()).is_ok());
    }

    #[test]
    fn preserves_populated_media_and_article_text_during_fallback() {
        let xml = nrk_feed("<media:description /><media:title>Train &amp; station</media:title>");
        let feed = parse_feed(xml.as_bytes()).unwrap();
        assert_eq!(
            feed.entries[0].media[0].title.as_ref().unwrap().content,
            "Train & station"
        );
        assert_eq!(
            feed.entries[0].summary.as_ref().unwrap().content,
            "Streiken fortsetter."
        );
        assert!(feed.entries[0].media[0].description.is_none());
    }

    #[test]
    fn normalization_resolves_namespaces_and_preserves_other_bytes() {
        let xml = br#"<rss xmlns:m="http://search.yahoo.com/mrss/" xmlns:media="urn:other"><m:title/><media:title/><title/><m:title><![CDATA[<m:title/>]]></m:title><m:description></m:description></rss>"#;
        let expected = br#"<rss xmlns:m="http://search.yahoo.com/mrss/" xmlns:media="urn:other"><media:title/><title/><m:title><![CDATA[<m:title/>]]></m:title></rss>"#;
        assert_eq!(without_empty_media_text(xml).unwrap(), expected);
    }

    #[test]
    fn malformed_xml_still_fails() {
        let xml = nrk_feed("<media:title />").replace("</item>", "</wrong>");
        assert!(parse_feed(xml.as_bytes()).is_err());
    }
}
