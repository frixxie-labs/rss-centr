//! Minimal client for Ollama's `/api/embed` endpoint.

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::semantic::normalize;

#[derive(Debug, Serialize)]
struct EmbedRequest<'a> {
    model: &'a str,
    input: &'a [String],
    truncate: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    keep_alive: Option<&'a str>,
}

#[derive(Debug, Deserialize)]
struct EmbedResponse {
    embeddings: Vec<Vec<f32>>,
}

/// Embed `inputs` with `model`, returning one unit-length vector per input.
///
/// `keep_alive` (for example `"1h"`) controls how long Ollama keeps the model
/// loaded afterwards; `None` uses Ollama's default of five minutes.
pub async fn embed(
    http: &reqwest::Client,
    ollama_url: &str,
    model: &str,
    inputs: &[String],
    keep_alive: Option<&str>,
) -> Result<Vec<Vec<f32>>> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!("{}/api/embed", ollama_url.trim_end_matches('/'));
    let response = http
        .post(&url)
        .json(&EmbedRequest {
            model,
            input: inputs,
            truncate: true,
            keep_alive,
        })
        .send()
        .await
        .with_context(|| format!("failed to call Ollama embed endpoint at {url}"))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .context("failed to read Ollama embed response")?;
    if !status.is_success() {
        bail!("Ollama embed with model {model} failed with status {status}: {body}");
    }
    parse_embed_response(&body, inputs.len())
}

/// Validate and normalize an Ollama embed response body.
pub fn parse_embed_response(body: &str, expected: usize) -> Result<Vec<Vec<f32>>> {
    let response: EmbedResponse =
        serde_json::from_str(body).context("failed to decode Ollama embed response")?;
    if response.embeddings.len() != expected {
        bail!(
            "Ollama returned {} embeddings for {expected} inputs",
            response.embeddings.len()
        );
    }
    let dimension = response.embeddings.first().map_or(0, Vec::len);
    response
        .embeddings
        .iter()
        .enumerate()
        .map(|(i, vector)| {
            if vector.len() != dimension {
                bail!(
                    "embedding {i} has dimension {} but expected {dimension}",
                    vector.len()
                );
            }
            normalize(vector).ok_or_else(|| anyhow!("embedding {i} is empty or degenerate"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Serve one HTTP request with the given status and body; returns the base
    /// URL and a handle resolving to the raw request that was received.
    async fn serve_once(
        status: &'static str,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = socket.read(&mut buf).await.unwrap();
                request.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&request);
                if let Some(header_end) = text.find("\r\n\r\n") {
                    let length = text[..header_end]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{addr}/"), handle)
    }

    #[test]
    fn parse_normalizes_vectors() {
        let vectors = parse_embed_response(r#"{"embeddings":[[3,4],[0,2]]}"#, 2).unwrap();
        assert_eq!(vectors, vec![vec![0.6, 0.8], vec![0.0, 1.0]]);
    }

    #[test]
    fn parse_rejects_wrong_count() {
        let err = parse_embed_response(r#"{"embeddings":[[1,0]]}"#, 2).unwrap_err();
        assert!(err.to_string().contains("1 embeddings for 2 inputs"));
    }

    #[test]
    fn parse_rejects_inconsistent_dimensions() {
        assert!(parse_embed_response(r#"{"embeddings":[[1,0],[1,0,0]]}"#, 2).is_err());
    }

    #[test]
    fn parse_rejects_degenerate_vectors_and_bad_json() {
        assert!(parse_embed_response(r#"{"embeddings":[[0,0]]}"#, 1).is_err());
        assert!(parse_embed_response(r#"{"embeddings":[[]]}"#, 1).is_err());
        assert!(parse_embed_response("not json", 1).is_err());
    }

    #[tokio::test]
    async fn embed_sends_model_and_inputs() {
        let (url, request) = serve_once("200 OK", r#"{"embeddings":[[1,0],[0,1]]}"#).await;
        let inputs = vec!["a".to_string(), "b".to_string()];
        let vectors = embed(
            &reqwest::Client::new(),
            &url,
            "embeddinggemma",
            &inputs,
            None,
        )
        .await
        .unwrap();
        assert_eq!(vectors.len(), 2);

        let request = request.await.unwrap();
        assert!(request.starts_with("POST /api/embed "));
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["model"], "embeddinggemma");
        assert_eq!(json["input"], serde_json::json!(["a", "b"]));
        assert!(
            json.get("keep_alive").is_none(),
            "Ollama's default applies when unset"
        );
    }

    #[tokio::test]
    async fn embed_sends_keep_alive_when_set() {
        let (url, request) = serve_once("200 OK", r#"{"embeddings":[[1,0]]}"#).await;
        embed(
            &reqwest::Client::new(),
            &url,
            "embeddinggemma",
            &["a".to_string()],
            Some("1h"),
        )
        .await
        .unwrap();

        let request = request.await.unwrap();
        let body = &request[request.find("\r\n\r\n").unwrap() + 4..];
        let json: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(json["keep_alive"], "1h");
    }

    #[tokio::test]
    async fn embed_reports_server_errors() {
        let (url, _request) = serve_once("500 Internal Server Error", r#"{"error":"boom"}"#).await;
        let err = embed(&reqwest::Client::new(), &url, "m", &["a".to_string()], None)
            .await
            .unwrap_err();
        let message = format!("{err:#}");
        assert!(message.contains("500"), "{message}");
        assert!(message.contains("boom"), "{message}");
    }

    #[tokio::test]
    async fn embed_with_no_inputs_skips_the_request() {
        let vectors = embed(
            &reqwest::Client::new(),
            "http://127.0.0.1:1",
            "m",
            &[],
            None,
        )
        .await
        .unwrap();
        assert!(vectors.is_empty());
    }
}
