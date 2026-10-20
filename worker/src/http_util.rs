use anyhow::{Context, Result, bail};
use reqwest::{RequestBuilder, Response};
use serde::de::DeserializeOwned;

/// Longest part of an error response body kept in error messages.
const MAX_ERROR_BODY_CHARS: usize = 500;

/// Sends `request`, turning transport failures and non-2xx responses into
/// errors. Error responses include (a prefix of) the response body.
pub(crate) async fn send(request: RequestBuilder, action: &str) -> Result<Response> {
    let response = request
        .send()
        .await
        .with_context(|| format!("failed to {action}"))?;
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().await.unwrap_or_default();
    bail!(
        "{action} failed with status {status}: {}",
        truncate_chars(&body, MAX_ERROR_BODY_CHARS)
    )
}

/// Like [`send`], then decodes the JSON response body.
pub(crate) async fn send_json<T: DeserializeOwned>(
    request: RequestBuilder,
    action: &str,
) -> Result<T> {
    send(request, action)
        .await?
        .json()
        .await
        .with_context(|| format!("failed to decode response to {action}"))
}

pub(crate) fn truncate_chars(text: &str, max_chars: usize) -> &str {
    text.char_indices()
        .nth(max_chars)
        .map_or(text, |(index, _)| &text[..index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_chars_respects_char_boundaries() {
        assert_eq!(truncate_chars("blåbær", 3), "blå");
        assert_eq!(truncate_chars("kort", 10), "kort");
        assert_eq!(truncate_chars("", 0), "");
    }
}
