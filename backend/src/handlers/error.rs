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
}

impl fmt::Display for HandlerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Error {}: {}", self.status, self.message)
    }
}

impl Error for HandlerError {}

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
}
