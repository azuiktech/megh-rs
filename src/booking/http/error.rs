//! Errors as Google AIP-193 describes them: `{"error": {"code": 404, "message": "...", "status": "NOT_FOUND"}}`.

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_extra::extract::QueryRejection;
use serde_json::json;

use crate::booking::Error;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    name: &'static str,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, name: &'static str, message: impl Into<String>) -> Self {
        Self { status, name, message: message.into() }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "INVALID_ARGUMENT", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"error": {"code": self.status.as_u16(), "message": self.message, "status": self.name}});
        (self.status, Json(body)).into_response()
    }
}

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        let message = error.to_string();
        match error {
            Error::NotFound => Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message),
            Error::Invalid(_) | Error::Recurrence(_) => Self::invalid(message),
            Error::NotOpen(_) | Error::HoldExpired(_) | Error::Insufficient(_) | Error::Cycle => Self::new(StatusCode::BAD_REQUEST, "FAILED_PRECONDITION", message),
            Error::Occupied(_) | Error::ClientBusy(_) => Self::new(StatusCode::CONFLICT, "ABORTED", message),
            Error::KeyReused => Self::new(StatusCode::CONFLICT, "ALREADY_EXISTS", message),
            Error::Database(error) => {
                tracing::error!(%error, "booking request failed");
                Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", "internal error")
            }
        }
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().is_some_and(|database| database.is_unique_violation()) {
            true => Self::new(StatusCode::CONFLICT, "ALREADY_EXISTS", "it exists already"),
            false => Error::Database(error).into(),
        }
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        Self::invalid(rejection.body_text())
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::invalid(rejection.body_text())
    }
}

impl From<PathRejection> for ApiError {
    fn from(rejection: PathRejection) -> Self {
        Self::invalid(rejection.body_text())
    }
}
