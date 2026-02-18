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
