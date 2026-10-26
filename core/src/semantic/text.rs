//! Building the text that is embedded for items and queries.

/// Upper bound on the number of characters sent to the embedding model per item.
pub const MAX_EMBEDDING_INPUT_CHARS: usize = 2000;

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
}
