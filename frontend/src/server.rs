use std::{
    sync::{Arc, LazyLock},
    time::{Duration, Instant},
};

use axum::{
    Json, Router,
    body::Body,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{any, get},
};
use leptos::prelude::{LeptosOptions, provide_context};
use leptos_axum::{LeptosRoutes, generate_route_list};
use serde_json::json;
use tower_http::compression::CompressionLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnFailure, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::{
    api::Api,
    app::{App, shell},
};

static STARTED: LazyLock<Instant> = LazyLock::new(Instant::now);

#[derive(Clone)]
pub struct Proxy {
    pub client: reqwest::Client,
    pub backend: String,
}

impl Proxy {
    pub fn from_env() -> Self {
        Self {
            client: reqwest::Client::new(),
            backend: std::env::var("BACKEND_URL")
                .unwrap_or_else(|_| "http://localhost:8080".into())
                .trim_end_matches('/')
                .into(),
        }
    }
}

pub fn router(options: LeptosOptions, proxy: Proxy) -> Router {
    LazyLock::force(&STARTED);
    tracing::info!(backend = %proxy.backend, "Starting frontend server");
    // Server-rendered pages call the backend directly; browsers go through the `/api` proxy.
    let api = Api::new(proxy.client.clone(), proxy.backend.clone());
    let routes = generate_route_list(App);
    let hashed_assets = options.hash_files;
    let router = Router::new()
        .leptos_routes_with_context(&options, routes, move || provide_context(api.clone()), {
            let options = options.clone();
            move || shell(options.clone())
        })
        .fallback(leptos_axum::file_and_error_handler(shell))
        .with_state(options)
        .merge(service_routes(proxy));
    // File names under /pkg change with their content when hashing is on.
    let router = if hashed_assets {
        router.layer(middleware::from_fn(cache_hashed_assets))
    } else {
        router
    };
    router
        // The default predicate skips text/event-stream, so SSE stays unbuffered.
        .layer(CompressionLayer::new())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO))
                .on_failure(DefaultOnFailure::new().level(Level::ERROR)),
        )
}

const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";

async fn cache_hashed_assets(request: Request, next: Next) -> Response {
    let hashed = request.uri().path().starts_with("/pkg/");
    let mut response = next.run(request).await;
    if hashed && response.status().is_success() {
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            header::HeaderValue::from_static(IMMUTABLE_CACHE),
        );
    }
    response
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

    async fn content_backend() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let item = json!({"id": 7, "feed_id": 1, "external_id": "x", "title": "Rust 2026 released",
            "url": "https://www.example.com/rust", "inserted_at": "2025-01-01T00:00:00Z",
            "summary": "<p>Big news</p>"});
        let app = Router::new()
            .route("/api/items/summary", get(|| async {
                Json(json!({"id": 1, "summary": "Today in Rust", "feed_ids": [1], "feed_item_ids": [7],
                    "generated_at": "2025-01-01T00:00:00Z", "model": "test-model"}))
            }))
            .route("/api/items/latest", get(move |uri: axum::http::Uri| {
                let item = item.clone();
                async move {
                    assert!(uri.query().unwrap_or_default().contains("limit=50"));
                    Json(json!([item]))
                }
            }))
            .route("/api/feeds", get(|| async {
                Json(json!([{"id": 1, "url": "https://example.com/feed.xml", "title": "Example Feed",
                    "poll_interval_seconds": 90, "is_enabled": true, "failure_count": 0}]))
            }))
            .route("/api/feeds/index/today", get(|| async {
                Json(json!([{"word": "ferris", "total_occurences": 3, "document_frequency": 2,
                    "tf_idf": 1.5, "items": [{"feed_src_id": 1, "occurences": 3}]}]))
            }));
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (url, task)
    }

    async fn page(app: &Router, path: &str) -> (StatusCode, String) {
        let response = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn pages_render_backend_data_on_the_server_with_islands() {
        let (backend, task) = content_backend().await;
        let options = LeptosOptions::builder()
            .output_name("rss-centr-frontend")
            .build();
        let app = router(
            options,
            Proxy {
                client: reqwest::Client::new(),
                backend,
            },
        );
        for (path, title, island, text) in [
            ("/", "AI Summary", "SummaryControls", "Today in Rust"),
            (
                "/timeline",
                "Live Timeline",
                "TimelineView",
                "Rust 2026 released",
            ),
            (
                "/news?q=rust&source_id=1",
                "All News",
                "NewsView",
                "Big news",
            ),
            ("/sources", "Sources", "SourcesView", "Example Feed"),
            ("/topics", "Topics", "TopicsView", "ferris"),
        ] {
            let (status, html) = page(&app, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(
                html.contains(&format!("RSS Centr - {title}")),
                "{path} title"
            );
            assert!(html.contains("<leptos-island"), "{path} island marker");
            assert!(html.contains(island), "{path} island {island}");
            assert!(html.contains(text), "{path} content");
        }
        let (status, html) = page(&app, "/does-not-exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(html.contains("Page not found."));
        task.abort();
    }

    #[tokio::test]
    async fn news_search_box_is_filled_in_server_html() {
        let (backend, task) = content_backend().await;
        let app = router(test_options(), test_proxy(backend));
        let (_, html) = page(&app, "/news?q=rust+%26+web&mode=keyword").await;
        assert!(html.contains(r#"value="rust &amp; web""#), "{html}");
        assert!(html.contains(r#"<option value="keyword" selected"#));
        task.abort();
    }

    #[tokio::test]
    async fn active_nav_link_is_marked_current() {
        let (backend, task) = content_backend().await;
        let app = router(test_options(), test_proxy(backend));
        let (_, html) = page(&app, "/sources").await;
        assert!(
            html.contains(r#"href="/sources" aria-current="page""#),
            "{html}"
        );
        assert_eq!(html.matches("aria-current").count(), 1);
        task.abort();
    }

    #[tokio::test]
    async fn failed_loads_show_errors_instead_of_empty_states() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let backend = format!("http://{}", listener.local_addr().unwrap());
        drop(listener);
        let app = router(test_options(), test_proxy(backend));
        for (path, error, empty) in [
            ("/timeline", "Could not load timeline.", "No news yet."),
            ("/news", "Could not load matching news.", "No news matches"),
            ("/sources", "Backend request failed", "No sources yet."),
            (
                "/topics",
                "Backend request failed",
                "No topic data available",
            ),
        ] {
            let (status, html) = page(&app, path).await;
            assert_eq!(status, StatusCode::OK, "{path}");
            assert!(html.contains(error), "{path} error");
            assert!(!html.contains(empty), "{path} empty state");
        }
    }

    #[tokio::test]
    async fn hashed_assets_are_cached_immutably_only_when_hashing() {
        let site = std::env::temp_dir().join(format!("rss-centr-site-{}", std::process::id()));
        std::fs::create_dir_all(site.join("pkg")).unwrap();
        std::fs::write(site.join("pkg/app.0123abcd.css"), "body{}").unwrap();
        std::fs::write(site.join("favicon.svg"), "<svg/>").unwrap();
        for hash_files in [true, false] {
            let options = LeptosOptions::builder()
                .output_name("rss-centr-frontend")
                .site_root(site.to_string_lossy().as_ref())
                .hash_files(hash_files)
                .build();
            let app = router(options, test_proxy("http://127.0.0.1:9".into()));
            for (path, immutable) in [
                ("/pkg/app.0123abcd.css", hash_files),
                ("/favicon.svg", false),
            ] {
                let response = app
                    .clone()
                    .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                assert_eq!(
                    response.headers().get(header::CACHE_CONTROL).is_some(),
                    immutable,
                    "{path} hash_files={hash_files}"
                );
                if immutable {
                    assert_eq!(response.headers()[header::CACHE_CONTROL], IMMUTABLE_CACHE);
                }
            }
        }
        std::fs::remove_dir_all(site).unwrap();
    }

    fn test_options() -> LeptosOptions {
        LeptosOptions::builder()
            .output_name("rss-centr-frontend")
            .build()
    }

    fn test_proxy(backend: String) -> Proxy {
        Proxy {
            client: reqwest::Client::new(),
            backend,
        }
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
