use axum::{Json, extract::State, http::StatusCode};
use rss_centr_core::summary_refresh::SummaryRefreshResponse;
use tracing::instrument;

use super::error::HandlerError;

pub(crate) use crate::summary::{DailySummary, SummaryState};

#[utoipa::path(
    get,
    path = "/api/items/summary",
    tag = "items",
    responses(
        (status = 200, description = "AI overview of items collected in the last 24 hours", body = DailySummary),
        (status = 404, description = "No summary is available yet"),
        (status = 500, description = "Database error"),
    )
)]
#[instrument(skip(state))]
pub async fn fetch_daily_summary(
    State(state): State<SummaryState>,
) -> Result<Json<DailySummary>, HandlerError> {
    state
        .latest_summary()
        .await?
        .map(Json)
        .ok_or_else(|| HandlerError::not_found("No summary is available yet."))
}

#[utoipa::path(
    post,
    path = "/internal/items/summary/refresh",
    tag = "items",
    responses(
        (status = 202, description = "Summary refresh accepted, already in progress, or not due yet", body = SummaryRefreshResponse),
        (status = 500, description = "Summary service unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn refresh_daily_summary(
    State(state): State<SummaryState>,
) -> Result<(StatusCode, Json<SummaryRefreshResponse>), HandlerError> {
    let next_due_at = state.refresh_scheduled().await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(SummaryRefreshResponse { next_due_at }),
    ))
}

#[utoipa::path(
    post,
    path = "/api/items/summary/generate",
    tag = "items",
    responses(
        (status = 202, description = "Summary generation started"),
        (status = 409, description = "Summary generation already in progress"),
        (status = 500, description = "Summary service unavailable"),
    )
)]
#[instrument(skip(state))]
pub async fn generate_daily_summary(
    State(state): State<SummaryState>,
) -> Result<StatusCode, HandlerError> {
    if state.start_refresh(true).await? {
        Ok(StatusCode::ACCEPTED)
    } else {
        Ok(StatusCode::CONFLICT)
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;

    use super::*;

    #[sqlx::test]
    async fn forced_generation_returns_conflict_for_an_active_lease(pool: PgPool) {
        sqlx::query(
            "UPDATE summary_refresh_queue SET lease_token = 'busy',
             lease_expires_at = NOW() + INTERVAL '1 hour'",
        )
        .execute(&pool)
        .await
        .unwrap();

        let status = generate_daily_summary(State(SummaryState::new(pool)))
            .await
            .unwrap();

        assert_eq!(status, StatusCode::CONFLICT);
    }
}
