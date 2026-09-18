use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing::{instrument, warn};
use utoipa::ToSchema;

use crate::analytics::{
    AnalyticsEventType, AnalyticsSummary, NewAnalyticsEvent, SummaryWindow, disabled_summary,
    fetch_summary, record_event,
};

use super::error::HandlerError;

#[derive(Clone)]
pub struct AnalyticsState {
    pub pool: PgPool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize, ToSchema)]
pub struct AnalyticsEventRequest {
    pub event_type: AnalyticsEventType,
    pub path: String,
    pub referrer: Option<String>,
    pub visitor_id: Option<String>,
    pub session_id: Option<String>,
    pub feed_id: Option<i64>,
    pub item_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AnalyticsSummaryQuery {
    pub days: Option<i64>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnalyticsEventAccepted {
    pub accepted: bool,
    pub enabled: bool,
}

#[utoipa::path(
    post,
    path = "/api/analytics/events",
    request_body = AnalyticsEventRequest,
    responses(
        (status = 202, description = "Analytics event accepted", body = AnalyticsEventAccepted),
        (status = 400, description = "Invalid input"),
        (status = 500, description = "Internal server error"),
    ),
    tag = "analytics"
)]
#[instrument(skip(state))]
pub async fn create_event(
    State(state): State<AnalyticsState>,
    Json(body): Json<AnalyticsEventRequest>,
) -> Result<(StatusCode, Json<AnalyticsEventAccepted>), HandlerError> {
    if !state.enabled {
        return Ok((
            StatusCode::ACCEPTED,
            Json(AnalyticsEventAccepted {
                accepted: false,
                enabled: false,
            }),
        ));
    }

    let event = NewAnalyticsEvent {
        event_type: body.event_type,
        path: normalize_path(&body.path)?,
        referrer: normalize_optional_text(body.referrer, 512),
        visitor_id: normalize_optional_text(body.visitor_id, 128),
        session_id: normalize_optional_text(body.session_id, 128),
        feed_id: body.feed_id,
        item_id: body.item_id,
    };

    record_event(&state.pool, &event).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to store analytics event")
    })?;

    Ok((
        StatusCode::ACCEPTED,
        Json(AnalyticsEventAccepted {
            accepted: true,
            enabled: true,
        }),
    ))
}

#[utoipa::path(
    get,
    path = "/api/analytics/summary",
    params(
        ("days" = Option<i64>, Query, description = "Rolling window size in days (1-90)")
    ),
    responses(
        (status = 200, description = "Analytics summary", body = AnalyticsSummary),
        (status = 500, description = "Internal server error"),
    ),
    tag = "analytics"
)]
#[instrument(skip(state))]
pub async fn get_summary(
    State(state): State<AnalyticsState>,
    Query(query): Query<AnalyticsSummaryQuery>,
) -> Result<Json<AnalyticsSummary>, HandlerError> {
    let window = SummaryWindow::new(query.days);

    if !state.enabled {
        return Ok(Json(disabled_summary(window)));
    }

    let summary = fetch_summary(&state.pool, window).await.map_err(|e| {
        warn!("failed with error: {e:#}");
        HandlerError::from_db(e, "Failed to load analytics summary")
    })?;

    Ok(Json(summary))
}

fn normalize_path(path: &str) -> Result<String, HandlerError> {
    let path = path.trim();
    if path.is_empty() || !path.starts_with('/') {
        return Err(HandlerError::bad_request(
            "analytics path must start with /",
        ));
    }
    if path.len() > 255 {
        return Err(HandlerError::bad_request("analytics path is too long"));
    }
    Ok(path.to_string())
}

fn normalize_optional_text(value: Option<String>, max_len: usize) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return None;
        }

        Some(trimmed.chars().take(max_len).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_requires_absolute_path() {
        assert!(normalize_path("/news").is_ok());
        assert!(normalize_path("news").is_err());
        assert!(normalize_path(" ").is_err());
    }

    #[test]
    fn test_normalize_optional_text_trims_and_limits() {
        assert_eq!(
            normalize_optional_text(Some("  ref  ".to_string()), 10),
            Some("ref".to_string())
        );
        assert_eq!(normalize_optional_text(Some("".to_string()), 10), None);
        assert_eq!(
            normalize_optional_text(Some("abcdef".to_string()), 3),
            Some("abc".to_string())
        );
    }

    #[test]
    fn test_summary_days_cap_matches_documented_limit() {
        assert_eq!(
            SummaryWindow::new(Some(crate::analytics::MAX_SUMMARY_DAYS + 50)).days,
            crate::analytics::MAX_SUMMARY_DAYS
        );
    }
}
