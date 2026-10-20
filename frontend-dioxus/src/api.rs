use std::sync::LazyLock;

use reqwest::{Client, Method, Response};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::{
    logic::{NewsFilters, slim_item},
    models::*,
};

static CLIENT: LazyLock<Client> = LazyLock::new(Client::new);

#[derive(Clone)]
pub struct Api {
    client: Client,
    base: String,
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use dioxus::server::axum::{
        Json, Router,
        http::StatusCode,
        routing::{get, put},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn handles_absent_summary_optional_details_and_camel_case_retry() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let count = attempts.clone();
        let app = Router::new()
            .route(
                "/api/items/summary",
                get(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/api/items/1",
                get(|| async {
                    Json(FeedItem {
                        id: 1,
                        title: "Fallback".into(),
                        ..Default::default()
                    })
                }),
            )
            .route(
                "/api/items/1/detail",
                get(|| async { StatusCode::NOT_FOUND }),
            )
            .route(
                "/api/feeds/1",
                put(move |Json(body): Json<Value>| {
                    let count = count.clone();
                    async move {
                        count.fetch_add(1, Ordering::SeqCst);
                        if body.get("isEnabled") == Some(&json!(false)) {
                            StatusCode::NO_CONTENT
                        } else {
                            StatusCode::BAD_REQUEST
                        }
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Api::new(
            Client::new(),
            format!("http://{}", listener.local_addr().unwrap()),
        );
        let server = tokio::spawn(async {
            dioxus::server::axum::serve(listener, app).await.unwrap();
        });
        assert_eq!(api.summary().await.unwrap(), None);
        assert_eq!(api.item(1).await.unwrap().title, "Fallback");
        api.enable_feed(1, false).await.unwrap();
        assert_eq!(attempts.load(Ordering::SeqCst), 2);
        let error = api.delete_feed(99).await.unwrap_err();
        assert!(error.contains("404"));
        server.abort();
    }
}

impl Default for Api {
    fn default() -> Self {
        #[cfg(all(feature = "web", target_arch = "wasm32"))]
        let base = web_sys::window()
            .and_then(|w| w.location().origin().ok())
            .unwrap_or_default();
        #[cfg(not(all(feature = "web", target_arch = "wasm32")))]
        let base = std::env::var("BACKEND_URL").unwrap_or_else(|_| "http://localhost:8080".into());
        Self::new(CLIENT.clone(), base)
    }
}

impl Api {
    pub fn new(client: Client, base: String) -> Self {
        Self {
            client,
            base: base.trim_end_matches('/').into(),
        }
    }

    async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Response, String> {
        let method_name = method.clone();
        let mut request = self
            .client
            .request(method, format!("{}/api/{path}", self.base));
        // Finite REST timeout; the streaming proxy deliberately has no overall timeout.
        #[cfg(not(target_arch = "wasm32"))]
        {
            request = request.timeout(std::time::Duration::from_secs(30));
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        tracing::debug!(%method_name, path, "Backend request");
        request.send().await.map_err(|error| {
            tracing::warn!(%method_name, path, %error, "Backend request failed");
            format!("Backend request failed: {error}")
        })
    }

    async fn checked(response: Response) -> Result<Response, String> {
        if response.status().is_success() {
            return Ok(response);
        }
        let status = response.status();
        let url = response.url().to_string();
        let body = response.text().await.unwrap_or_default();
        tracing::warn!(%status, %url, %body, "Backend returned error status");
        Err(format!("Request failed: {status} ({body})"))
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, String> {
        Self::checked(self.request(Method::GET, path, None).await?)
            .await?
            .json()
            .await
            .map_err(|error| {
                tracing::warn!(path, %error, "Invalid backend response");
                format!("Invalid backend response: {error}")
            })
    }

    pub async fn summary(&self) -> Result<Option<DailySummary>, String> {
        let response = self.request(Method::GET, "items/summary", None).await?;
        if response.status() == 404 {
            return Ok(None);
        }
        Self::checked(response)
            .await?
            .json()
            .await
            .map(Some)
            .map_err(|error| {
                tracing::warn!(%error, "Invalid summary response");
                error.to_string()
            })
    }

    pub async fn items(
        &self,
        filters: &NewsFilters,
        offset: usize,
    ) -> Result<Vec<FeedItem>, String> {
        let items: Vec<FeedItem> = self
            .get(&format!("items/latest?{}", filters.api_query(offset)))
            .await?;
        Ok(items.into_iter().map(slim_item).collect())
    }

    pub async fn item(&self, id: i64) -> Result<FeedItem, String> {
        let item_path = format!("items/{id}");
        let detail_path = format!("items/{id}/detail");
        let (item, detail) = futures_util::join!(
            self.get::<FeedItem>(&item_path),
            self.get::<ItemDetail>(&detail_path)
        );
        let mut item = item?;
        if let Ok(detail) = detail {
            item.summary = Some(detail.summary);
            item.content = Some(detail.content);
            item.author = Some(detail.author);
            item.published_at = Some(detail.published_at);
        }
        Ok(slim_item(item))
    }

    pub async fn feeds(&self) -> Result<Vec<Feed>, String> {
        self.get("feeds").await
    }
    pub async fn topics(&self) -> Result<Vec<Topic>, String> {
        self.get("feeds/index/today").await
    }

    pub async fn create_feed(&self, url: &str) -> Result<(), String> {
        Self::checked(
            self.request(Method::POST, "feeds", Some(json!({"url": url})))
                .await?,
        )
        .await?;
        Ok(())
    }

    pub async fn enable_feed(&self, id: i64, enabled: bool) -> Result<(), String> {
        let path = format!("feeds/{id}");
        let mut response = self
            .request(Method::PUT, &path, Some(json!({"is_enabled": enabled})))
            .await?;
        if response.status() == 400 {
            response = self
                .request(Method::PUT, &path, Some(json!({"isEnabled": enabled})))
                .await?;
        }
        Self::checked(response).await?;
        Ok(())
    }

    pub async fn generate_summary(&self) -> Result<bool, String> {
        let response = self
            .request(Method::POST, "items/summary/generate", None)
            .await?;
        if response.status() == 409 {
            return Ok(false);
        }
        Self::checked(response).await?;
        Ok(true)
    }

    pub async fn ingest(&self, id: i64) -> Result<(), String> {
        Self::checked(
            self.request(Method::POST, &format!("feeds/{id}/ingest"), None)
                .await?,
        )
        .await?;
        Ok(())
    }

    pub async fn delete_feed(&self, id: i64) -> Result<(), String> {
        Self::checked(
            self.request(Method::DELETE, &format!("feeds/{id}"), None)
                .await?,
        )
        .await?;
        Ok(())
    }
}
