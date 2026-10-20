use std::{
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use dioxus::server::axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get},
};
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::app::App;

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Clone)]
pub struct Proxy {
    pub client: reqwest::Client,
    pub backend: String,
}

pub fn router() -> Router {
    LazyLock::force(&STARTED);
    let proxy = Proxy {
        client: reqwest::Client::new(),
        backend: std::env::var("BACKEND_URL")
            .unwrap_or_else(|_| "http://localhost:8080".into())
            .trim_end_matches('/')
            .into(),
    };
    tracing::info!(backend = %proxy.backend, "Starting frontend server");
    dioxus::server::router(App)
        .merge(service_routes(proxy))
        // The default predicate skips text/event-stream, so SSE stays unbuffered.
        .layer(CompressionLayer::new())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO))
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
}

pub fn service_routes(proxy: Proxy) -> Router {
    Router::new()
        .route("/api/{*path}", any(forward))
        .route("/status/ping", get(ping))
        .route("/status/health", get(health))
        .route("/health", get(health))
        .route("/feeds", get(legacy_redirect))
        .route("/items", get(legacy_redirect))
        .route("/index-words", get(legacy_redirect))
        .with_state(Arc::new(proxy))
}

async fn ping() -> Json<serde_json::Value> {
    Json(json!({"status": "ok", "timestamp": chrono::Utc::now().to_rfc3339()}))
}

async fn health(State(proxy): State<Arc<Proxy>>) -> impl IntoResponse {
    let ok = proxy
        .client
        .get(format!("{}/status/ping", proxy.backend))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .is_ok_and(|r| r.status().is_success());
    if !ok {
        tracing::warn!(backend = %proxy.backend, "Health check: backend unreachable");
    }
    (
        if ok {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        Json(json!({
            "status": if ok { "ok" } else { "degraded" },
            "backend": if ok { "ok" } else { "unreachable" },
            "uptime": STARTED.elapsed().as_secs(),
            "timestamp": chrono::Utc::now().to_rfc3339(),
        })),
    )
}

async fn legacy_redirect(OriginalUri(uri): OriginalUri) -> Redirect {
    let path = match uri.path() {
        "/feeds" => "/sources",
        "/items" => "/news",
        _ => "/topics",
    };
    let target = match uri.query() {
        Some(q) => format!("{path}?{q}"),
        None => path.into(),
    };
    Redirect::permanent(&target)
}

pub fn response_headers(headers: &HeaderMap) -> HeaderMap {
    let connection_headers: Vec<_> = headers
        .get_all(header::CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|s| s.split(','))
        .map(|s| s.trim().to_ascii_lowercase())
        .collect();
    headers
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "connection"
                    | "keep-alive"
                    | "proxy-authenticate"
                    | "proxy-authorization"
                    | "te"
                    | "trailer"
                    | "trailers"
                    | "transfer-encoding"
                    | "upgrade"
            ) && !connection_headers.iter().any(|n| n == name.as_str())
        })
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

async fn forward(State(proxy): State<Arc<Proxy>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let target = format!(
        "{}{}",
        proxy.backend,
        parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
    );
    let mut headers = HeaderMap::new();
    for key in [
        "accept",
        "authorization",
        "cache-control",
        "content-type",
        "if-match",
        "if-modified-since",
        "if-none-match",
        "last-event-id",
    ] {
        if let Some(value) = parts.headers.get(key) {
            headers.insert(key, value.clone());
        }
    }
    let result = proxy
        .client
        .request(parts.method, target)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    match result {
        Ok(upstream) => {
            let status = upstream.status();
            let mut headers = response_headers(upstream.headers());
            if headers
                .get(header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("text/event-stream"))
            {
                headers.insert(
                    header::CACHE_CONTROL,
                    header::HeaderValue::from_static("no-cache"),
                );
                headers.insert("x-accel-buffering", header::HeaderValue::from_static("no"));
            }
            let mut response = Body::from_stream(upstream.bytes_stream()).into_response();
            *response.status_mut() = status;
            *response.headers_mut() = headers;
            response
        }
        Err(error) => {
            tracing::warn!(%error, "Backend proxy failed");
            (StatusCode::BAD_GATEWAY, "Bad Gateway").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dioxus::server::axum;
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    async fn mock_backend() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new()
            .route("/status/ping", get(|| async { "ok" }))
            .route("/api/echo", any(|req: Request| async move {
                let method = req.method().to_string();
                let query = req.uri().query().unwrap_or_default().to_owned();
                let cursor = req.headers().get("last-event-id").cloned();
                let body = req.into_body().collect().await.unwrap().to_bytes();
                Json(json!({"method": method, "query": query, "cursor": cursor.and_then(|v| v.to_str().ok().map(str::to_owned)), "body": String::from_utf8_lossy(&body)}))
            }))
            .route("/api/stream", get(|| async {
                axum::response::Sse::new(futures_stream()).keep_alive(axum::response::sse::KeepAlive::default())
            }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, task)
    }

    fn futures_stream()
    -> impl futures_util::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>
    {
        async_stream::stream! {
            yield Ok(axum::response::sse::Event::default().event("feed_item").id("42").data("hello"));
            std::future::pending::<()>().await;
        }
    }

    #[tokio::test]
    async fn proxy_preserves_method_query_cursor_body_and_status() {
        let (backend, task) = mock_backend().await;
        let app = service_routes(Proxy {
            client: reqwest::Client::new(),
            backend,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/echo?q=rust%20web")
                    .header("last-event-id", "99")
                    .body(Body::from("payload"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(
            body,
            json!({"method": "PUT", "query": "q=rust%20web", "cursor": "99", "body": "payload"})
        );
        task.abort();
    }

    #[tokio::test]
    async fn sse_first_chunk_arrives_without_waiting_for_stream_to_finish() {
        let (backend, task) = mock_backend().await;
        let app = service_routes(Proxy {
            client: reqwest::Client::new(),
            backend,
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/stream")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.headers()["x-accel-buffering"], "no");
        let mut body = response.into_body();
        let frame = tokio::time::timeout(Duration::from_secs(2), body.frame())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .into_data()
            .unwrap();
        assert!(String::from_utf8_lossy(&frame).contains("42"));
        task.abort();
    }

    #[tokio::test]
    async fn health_aliases_and_redirects_keep_contracts() {
        let (backend, task) = mock_backend().await;
        let app = service_routes(Proxy {
            client: reqwest::Client::new(),
            backend,
        });
        for path in ["/health", "/status/health", "/status/ping"] {
            let response = app
                .clone()
                .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
        }
        for (from, to) in [
            ("/feeds", "/sources"),
            ("/items", "/news"),
            ("/index-words", "/topics"),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{from}?q=rust"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
            assert_eq!(response.headers()["location"], format!("{to}?q=rust"));
        }
        task.abort();
    }

    #[tokio::test]
    async fn unreachable_backend_is_502_and_health_is_degraded() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let app = service_routes(Proxy {
            client: reqwest::Client::new(),
            backend,
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/feeds")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body: serde_json::Value =
            serde_json::from_slice(&response.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(body["status"], "degraded");
        assert_eq!(body["backend"], "unreachable");
    }

    #[test]
    fn hop_by_hop_and_connection_named_headers_are_removed() {
        let mut headers = HeaderMap::new();
        headers.insert("connection", "keep-alive, x-private".parse().unwrap());
        headers.insert("x-private", "secret".parse().unwrap());
        headers.insert("content-type", "text/event-stream".parse().unwrap());
        let filtered = response_headers(&headers);
        assert!(!filtered.contains_key("connection"));
        assert!(!filtered.contains_key("x-private"));
        assert!(filtered.contains_key("content-type"));
    }
}
