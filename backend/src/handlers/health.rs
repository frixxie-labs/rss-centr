use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tracing::{instrument, warn};
use utoipa::ToSchema;

#[derive(Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    pub status: String,
    pub timestamp: String,
    pub checks: HealthChecks,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct HealthChecks {
    pub database: CheckResult,
}

#[derive(Serialize, Deserialize, ToSchema)]
pub struct CheckResult {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[utoipa::path(
    get,
    path = "/status/health",
    responses(
        (status = 200, description = "Service is healthy", body = HealthResponse),
        (status = 503, description = "Service is unhealthy", body = HealthResponse),
    ),
    tag = "system"
)]
#[instrument(skip(pool))]
pub async fn health(State(pool): State<SqlitePool>) -> impl IntoResponse {
    let timestamp = chrono::Utc::now().to_rfc3339();

    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&pool)
        .await
    {
        Ok(_) => (
            StatusCode::OK,
            Json(HealthResponse {
                status: "healthy".to_string(),
                timestamp,
                checks: HealthChecks {
                    database: CheckResult {
                        status: "ok".to_string(),
                        message: None,
                    },
                },
            }),
        ),
        Err(e) => {
            warn!("Health check failed: database unreachable: {e:#}");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(HealthResponse {
                    status: "unhealthy".to_string(),
                    timestamp,
                    checks: HealthChecks {
                        database: CheckResult {
                            status: "fail".to_string(),
                            message: Some("database unreachable".to_string()),
                        },
                    },
                }),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use sqlx::SqlitePool;
    use tower::ServiceExt;

    fn app(pool: SqlitePool) -> Router {
        Router::new()
            .route("/status/health", get(health))
            .with_state(pool)
    }

    #[sqlx::test]
    async fn test_health_returns_200_when_db_is_reachable(pool: SqlitePool) {
        let response = app(pool)
            .oneshot(
                Request::builder()
                    .uri("/status/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(health.status, "healthy");
        assert_eq!(health.checks.database.status, "ok");
        assert!(health.checks.database.message.is_none());
        assert!(!health.timestamp.is_empty());
    }

    #[sqlx::test]
    async fn test_health_response_timestamp_is_rfc3339(pool: SqlitePool) {
        let response = app(pool)
            .oneshot(
                Request::builder()
                    .uri("/status/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();

        chrono::DateTime::parse_from_rfc3339(&health.timestamp)
            .expect("timestamp should be valid RFC 3339");
    }

    #[sqlx::test]
    async fn test_health_response_content_type_is_json(pool: SqlitePool) {
        let response = app(pool)
            .oneshot(
                Request::builder()
                    .uri("/status/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("application/json"),
            "expected application/json, got {content_type}"
        );
    }

    #[tokio::test]
    async fn test_health_returns_503_when_db_is_closed() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap();
        pool.close().await;

        let response = app(pool)
            .oneshot(
                Request::builder()
                    .uri("/status/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let health: HealthResponse = serde_json::from_slice(&body).unwrap();

        assert_eq!(health.status, "unhealthy");
        assert_eq!(health.checks.database.status, "fail");
        assert_eq!(
            health.checks.database.message.as_deref(),
            Some("database unreachable")
        );
    }
}
