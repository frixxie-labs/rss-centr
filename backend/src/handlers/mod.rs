use axum::{
    Router,
    extract::MatchedPath,
    extract::Request,
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post, put},
};
use metrics::histogram;
use metrics_exporter_prometheus::PrometheusHandle;
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tokio::sync::mpsc::Sender;
use tokio::time::Instant;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::{info, instrument};
use utoipa::OpenApi;

use crate::background_tasks::IngestJob;
use crate::events::NewFeedItemEvent;

mod error;
mod feeds;
mod items;
mod ping;
mod sse;

#[instrument]
pub async fn profile_endpoint(request: Request, next: Next) -> Response {
    let method = request.method().clone().to_string();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map(MatchedPath::as_str)
        .unwrap_or(request.uri().path())
        .to_string();
    info!("Handling {method} at {route}");

    let now = Instant::now();
    let labels = [("method", method.clone()), ("route", route.clone())];

    let response = next.run(request).await;
    let elapsed = now.elapsed();
    histogram!("handler", &labels).record(elapsed);

    info!(
        "Finished handling {} at {}, used {} ms",
        method,
        route,
        elapsed.as_millis()
    );
    response
}

pub fn create_router(
    pool: SqlitePool,
    metrics_handler: PrometheusHandle,
    tx: Sender<IngestJob>,
    new_item_tx: broadcast::Sender<NewFeedItemEvent>,
) -> Router {
    let feeds = Router::new()
        .route("/feeds", get(feeds::fetch_feeds))
        .route("/feeds", post(feeds::create_feed))
        .route("/feeds/{feed_id}", get(feeds::fetch_feed_by_id))
        .route("/feeds/{feed_id}", put(feeds::update_feed_enabled))
        .route("/feeds/{feed_id}", delete(feeds::delete_feed))
        .route("/feeds/{feed_id}/ingest", post(feeds::queue_ingest_feed))
        .with_state((pool.clone(), tx.clone()));

    let items = Router::new()
        .route("/feeds/{feed_id}/items", get(items::fetch_items_by_feed))
        .route("/items/latest", get(items::fetch_latest_items))
        .route("/items/{item_id}", get(items::fetch_item_by_id))
        .route("/items/{item_id}/detail", get(items::fetch_item_detail))
        .with_state(pool.clone());

    let item_events = Router::new()
        .route("/items/stream", get(sse::stream_new_items))
        .with_state((pool.clone(), new_item_tx));

    Router::new()
        .nest("/api", feeds)
        .nest("/api", items)
        .nest("/api", item_events)
        .route("/status/ping", get(ping::ping))
        .route("/metrics", get(metrics))
        .route("/openapi", get(get_openapi))
        .with_state(metrics_handler)
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(middleware::from_fn(profile_endpoint)),
        )
}

#[utoipa::path(
    get,
    path = "/metrics",
    responses(
        (status = 200, description = "Prometheus metrics in exposition format", body = String),
    )
)]
#[instrument]
async fn metrics(axum::extract::State(handle): axum::extract::State<PrometheusHandle>) -> String {
    handle.render()
}

#[derive(OpenApi)]
#[openapi(
    paths(
        metrics,
        ping::ping,
        feeds::fetch_feeds,
        feeds::create_feed,
        feeds::fetch_feed_by_id,
        feeds::update_feed_enabled,
        feeds::delete_feed,
        feeds::queue_ingest_feed,
        items::fetch_items_by_feed,
        items::fetch_latest_items,
        items::fetch_item_by_id,
        items::fetch_item_detail,
    ),
    components(
        schemas(
            crate::feed::feed_subscription::FeedSubscription,
            feeds::NewFeed,
            feeds::UpdateFeedEnabled,
            crate::feed::feed_item::FeedItem,
            crate::feed::feed_item::FeedItemWithDetail,
            crate::feed::feed_item::FeedItemDetail,
            ping::PingResponse,
        )
    ),
    tags(
        (name = "feeds", description = "Feed subscription endpoints"),
        (name = "items", description = "Feed item endpoints"),
        (name = "system", description = "System health and metrics endpoints"),
    )
)]
pub struct ApiDoc;

pub async fn get_openapi() -> String {
    let doc = ApiDoc::openapi();
    serde_json::to_string_pretty(&doc).unwrap_or_else(|e| {
        tracing::error!("failed to render openapi document: {e}");
        "{}".to_string()
    })
}
