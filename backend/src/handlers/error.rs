use std::{error::Error, fmt};

use axum::{http::StatusCode, response::IntoResponse};
use sqlx::Error as SqlxError;

#[derive(Debug, Clone)]
pub struct HandlerError {
    pub status: StatusCode,
    pub message: String,
}

impl HandlerError {
    pub fn new(status: StatusCode, message: String) -> Self {
        Self { status, message }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message.into())
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, message.into())
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, message.into())
    }

    pub fn from_db(err: anyhow::Error, internal_message: impl Into<String>) -> Self {
        if is_not_found_error(&err) {
            return Self::not_found(err.to_string());
        }

        Self::internal(format!("{}: {err}", internal_message.into()))
    }

    pub fn from_feed_queue(
        error: crate::feed::feed_update_queue::FeedQueueError,
        internal_message: impl Into<String>,
    ) -> Self {
        use crate::feed::feed_update_queue::FeedQueueError;

        match error {
            FeedQueueError::LeaseConflict { .. } => Self::conflict(error.to_string()),
            FeedQueueError::FeedNotFound { .. } => Self::not_found(error.to_string()),
            FeedQueueError::FeedPaused { .. } => {
                Self::internal(format!("{}: {error}", internal_message.into()))
            }
            FeedQueueError::Storage(error) => {
                Self::internal(format!("{}: {error}", internal_message.into()))
            }
        }
    }
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error {}: {}", self.status, self.message)
    }
}

impl Error for HandlerError {}

impl From<crate::search::SearchError> for HandlerError {
    fn from(error: crate::search::SearchError) -> Self {
        match error {
            crate::search::SearchError::Storage(error) => {
                tracing::warn!("failed with error: {error:#}");
                Self::from_db(error, "Failed to fetch data from database")
            }
            crate::search::SearchError::EmbeddingUnavailable => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "Semantic search is unavailable".to_string(),
            ),
        }
    }
}

impl From<crate::summary::InputTooLarge> for HandlerError {
    fn from(error: crate::summary::InputTooLarge) -> Self {
        tracing::error!(%error, "single summary input exceeds context budget");
        Self::internal(error.to_string())
    }
}

impl From<crate::summary::SummaryStoreError> for HandlerError {
    fn from(error: crate::summary::SummaryStoreError) -> Self {
        tracing::error!(error = ?error, "summary persistence failed");
        Self::internal(error.to_string())
    }
}

impl From<crate::summary::SummaryError> for HandlerError {
    fn from(error: crate::summary::SummaryError) -> Self {
        use crate::summary::SummaryError;

        let status = match error {
            SummaryError::Store(error) => return Self::from(error),
            SummaryError::InputTooLarge(error) => return Self::from(error),
            SummaryError::NoArticles => StatusCode::NOT_FOUND,
            SummaryError::MergeBudgetTooSmall => StatusCode::INTERNAL_SERVER_ERROR,
            SummaryError::ModelUnavailable
            | SummaryError::InvalidModelMetadata
            | SummaryError::MissingContextLimit
            | SummaryError::ServiceUnavailable
            | SummaryError::InvalidResponse
            | SummaryError::ContextOverflow
            | SummaryError::Truncated
            | SummaryError::Incomplete
            | SummaryError::EmptyResponse => StatusCode::BAD_GATEWAY,
        };
        Self::new(status, error.to_string())
    }
}

impl IntoResponse for HandlerError {
    fn into_response(self) -> axum::response::Response {
        (self.status, self.message).into_response()
    }
}

fn is_not_found_error(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<SqlxError>()
            .is_some_and(|sqlx_err| matches!(sqlx_err, SqlxError::RowNotFound))
            || cause.to_string().starts_with("no ")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feed_queue_domain_errors_preserve_http_messages_and_statuses() {
        use crate::feed::feed_update_queue::FeedQueueError;

        for (error, status, message) in [
            (
                FeedQueueError::LeaseConflict { feed_id: 7 },
                StatusCode::CONFLICT,
                "feed update lease conflict for feed_id=7",
            ),
            (
                FeedQueueError::FeedNotFound { feed_id: 7 },
                StatusCode::NOT_FOUND,
                "no feed found with id=7",
            ),
            (
                FeedQueueError::FeedPaused { feed_id: 7 },
                StatusCode::INTERNAL_SERVER_ERROR,
                "Queue failed: feed is paused for feed_id=7",
            ),
            (
                FeedQueueError::Storage(anyhow::anyhow!("database unavailable")),
                StatusCode::INTERNAL_SERVER_ERROR,
                "Queue failed: database unavailable",
            ),
        ] {
            let error = HandlerError::from_feed_queue(error, "Queue failed");
            assert_eq!(error.status, status);
            assert_eq!(error.message, message);
        }
    }

    #[test]
    fn search_errors_preserve_http_statuses_and_messages() {
        let unavailable = HandlerError::from(crate::search::SearchError::EmbeddingUnavailable);
        assert_eq!(unavailable.status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(unavailable.message, "Semantic search is unavailable");

        let storage = HandlerError::from(crate::search::SearchError::Storage(anyhow::anyhow!(
            "storage unavailable"
        )));
        assert_eq!(storage.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            storage.message,
            "Failed to fetch data from database: storage unavailable"
        );

        let missing = HandlerError::from(crate::search::SearchError::Storage(
            anyhow::Error::new(sqlx::Error::RowNotFound).context("missing item"),
        ));
        assert_eq!(missing.status, StatusCode::NOT_FOUND);
        assert_eq!(missing.message, "missing item");
    }

    #[test]
    fn oversized_summary_input_preserves_http_error() {
        let error = HandlerError::from(crate::summary::InputTooLarge);
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.message, "Summary input exceeds model context budget");
    }

    #[test]
    fn summary_storage_errors_preserve_http_messages_without_exposing_database_details() {
        for (error, message) in [
            (
                crate::summary::SummaryStoreError::LeaseConflict,
                "Summary refresh lease conflict",
            ),
            (
                crate::summary::SummaryStoreError::Database {
                    operation: "Failed to save summary",
                    source: sqlx::Error::Protocol("private database details".into()),
                },
                "Failed to save summary",
            ),
        ] {
            let error = HandlerError::from(error);
            assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(error.message, message);
        }
    }

    #[test]
    fn summary_generation_errors_preserve_http_statuses_and_messages() {
        use crate::summary::SummaryError;

        for (error, status, message) in [
            (
                SummaryError::NoArticles,
                StatusCode::NOT_FOUND,
                "No recent articles available for summary generation.",
            ),
            (
                SummaryError::MergeBudgetTooSmall,
                StatusCode::INTERNAL_SERVER_ERROR,
                "Summary context budget too small to merge summaries",
            ),
            (
                SummaryError::ModelUnavailable,
                StatusCode::BAD_GATEWAY,
                "Summary model unavailable",
            ),
            (
                SummaryError::InvalidModelMetadata,
                StatusCode::BAD_GATEWAY,
                "Invalid summary model metadata",
            ),
            (
                SummaryError::MissingContextLimit,
                StatusCode::BAD_GATEWAY,
                "Summary model context limit missing",
            ),
            (
                SummaryError::ServiceUnavailable,
                StatusCode::BAD_GATEWAY,
                "Summary service unavailable",
            ),
            (
                SummaryError::InvalidResponse,
                StatusCode::BAD_GATEWAY,
                "Invalid summary response",
            ),
            (
                SummaryError::ContextOverflow,
                StatusCode::BAD_GATEWAY,
                "Summary generation exceeded context budget",
            ),
            (
                SummaryError::Truncated,
                StatusCode::BAD_GATEWAY,
                "Summary generation truncated",
            ),
            (
                SummaryError::Incomplete,
                StatusCode::BAD_GATEWAY,
                "Summary generation incomplete",
            ),
            (
                SummaryError::EmptyResponse,
                StatusCode::BAD_GATEWAY,
                "Empty summary response",
            ),
            (
                SummaryError::InputTooLarge(crate::summary::InputTooLarge),
                StatusCode::INTERNAL_SERVER_ERROR,
                "Summary input exceeds model context budget",
            ),
            (
                SummaryError::Store(crate::summary::SummaryStoreError::LeaseConflict),
                StatusCode::INTERNAL_SERVER_ERROR,
                "Summary refresh lease conflict",
            ),
        ] {
            let error = HandlerError::from(error);
            assert_eq!(error.status, status);
            assert_eq!(error.message, message);
        }
    }
}
