//! What messaging can fail with, and the HTTP status of each.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::auth::rest::ApiError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MessagingError {
    #[error("not found")]
    NotFound,
    #[error("the attachment is over the limit of {limit} bytes")]
    TooLarge { limit: usize },
    #[error("not an a2ui v1.0 surface: {0}")]
    InvalidSurface(String),
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Token(#[from] jsonwebtoken::errors::Error),
}

impl From<ApiError> for MessagingError {
    fn from(error: ApiError) -> Self {
        match error {
            ApiError::NotFound => Self::NotFound,
            ApiError::Database(error) => Self::Database(error),
            ApiError::BadRequest(message) | ApiError::Conflict(message) => Self::BadRequest(message),
        }
    }
}

impl IntoResponse for MessagingError {
    fn into_response(self) -> Response {
        match self {
            Self::Database(error) => ApiError::Database(error).into_response(),
            Self::NotFound => ApiError::NotFound.into_response(),
            Self::TooLarge { .. } => (StatusCode::PAYLOAD_TOO_LARGE, self.to_string()).into_response(),
            Self::InvalidSurface(_) | Self::BadRequest(_) => (StatusCode::BAD_REQUEST, self.to_string()).into_response(),
            Self::Token(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        }
    }
}
