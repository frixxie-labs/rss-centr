use axum::Json;
use serde::Serialize;
use utoipa::ToSchema;

use super::error::HandlerError;

#[derive(Serialize, ToSchema)]
pub struct PingResponse {
    pub status: String,
    pub timestamp: String,
}

#[utoipa::path(
    get,
    path = "/status/ping",
    responses(
        (status = 200, description = "Health check successful", body = PingResponse),
    ),
    tag = "system"
)]
pub async fn ping() -> Result<Json<PingResponse>, HandlerError> {
    let timestamp = chrono::Utc::now().to_rfc3339();
    Ok(Json(PingResponse {
        status: "ok".to_string(),
        timestamp,
    }))
}
