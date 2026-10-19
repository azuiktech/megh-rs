//! What every REST API in this crate shares: the error as Google AIP-193 describes it, the `ApiResult` a handler
//! returns, the query and page of a List (AIP-132, 158), and the middleware that writes responses in the format the
//! request's `Accept` header asks for.

use axum::extract::rejection::{JsonRejection, PathRejection};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use axum_content_negotiation::Negotiate;
use axum_extra::extract::QueryRejection;
use serde::{Deserialize, Serialize};

use crate::Entity;

pub use axum_content_negotiation::NegotiateLayer;

/// What a handler returns: the value, or the error written as AIP-193.
pub type ApiResult<T, E = ApiError> = Result<T, E>;

/// An error as AIP-193 writes it: `{"error": {"code": 404, "message": "...", "status": "NOT_FOUND"}}`.
#[derive(Serialize, utoipa::ToSchema)]
pub struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Serialize, utoipa::ToSchema)]
struct ErrorDetail {
    code: u16,
    message: String,
    /// `INVALID_ARGUMENT`, `NOT_FOUND`, `ALREADY_EXISTS`, `FAILED_PRECONDITION`, `ABORTED` or `INTERNAL`.
    status: &'static str,
}

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    name: &'static str,
    message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, name: &'static str, message: impl Into<String>) -> Self {
        Self { status, name, message: message.into() }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "INVALID_ARGUMENT", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "ALREADY_EXISTS", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message)
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        tracing::error!(%error, "request failed");
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL", "internal error")
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody { error: ErrorDetail { code: self.status.as_u16(), message: self.message, status: self.name } };
        (self.status, Json(body)).into_response()
    }
}

#[cfg(feature = "postgres")]
impl From<sqlx::Error> for ApiError {
    fn from(error: sqlx::Error) -> Self {
        match error.as_database_error().is_some_and(|database| database.is_unique_violation()) {
            true => Self::conflict("it exists already"),
            false => Self::internal(error),
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

impl From<validator::ValidationErrors> for ApiError {
    fn from(errors: validator::ValidationErrors) -> Self {
        Self::invalid(errors.to_string())
    }
}

/// Errors that axum or an extractor writes as plain text are rewritten as AIP-193 bodies; `ApiError` already is one.
pub async fn aip_193(response: Response) -> Response {
    let (parts, body) = response.into_parts();
    let is_json = parts.headers.get(axum::http::header::CONTENT_TYPE).is_some_and(|kind| kind.as_bytes().starts_with(b"application/json"));
    if is_json || !(parts.status.is_client_error() || parts.status.is_server_error()) {
        return Response::from_parts(parts, body);
    }
    let message = axum::body::to_bytes(body, usize::MAX).await.map(|bytes| String::from_utf8_lossy(&bytes).into_owned()).unwrap_or_default();
    let name = match parts.status {
        StatusCode::NOT_FOUND => "NOT_FOUND",
        StatusCode::UNAUTHORIZED => "UNAUTHENTICATED",
        StatusCode::FORBIDDEN => "PERMISSION_DENIED",
        StatusCode::INTERNAL_SERVER_ERROR => "INTERNAL",
        _ => "INVALID_ARGUMENT",
    };
    ApiError::new(parts.status, name, message).into_response()
}

/// The query of a List (AIP-132, 158, 160, 164).
#[serde_with::serde_as]
#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct ListQuery {
    /// At most this many items; 50 when absent, 1000 at most.
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub page_size: Option<i64>,
    /// The `next_page_token` of the previous page.
    pub page_token: Option<String>,
    /// An AIP-160 expression over the fields, such as `capacity > 1`.
    pub filter: Option<String>,
    /// Fields with an optional `desc`, such as `capacity desc, name`.
    pub order_by: Option<String>,
    /// Include deleted items.
    #[serde_as(as = "Option<serde_with::DisplayFromStr>")]
    pub show_deleted: Option<bool>,
}

/// One page of items and the token of the next (empty on the last page).
#[derive(Serialize, utoipa::ToSchema)]
pub struct Paginated<T> {
    pub data: Vec<T>,
    pub next_page_token: String,
}

impl<ID: Serialize + Send + Sync + 'static, T: Serialize + Send + Sync + 'static> IntoResponse for Entity<ID, T> {
    fn into_response(self) -> Response {
        Negotiate(self).into_response()
    }
}

impl<T: Serialize + Send + Sync + 'static> IntoResponse for Paginated<T> {
    fn into_response(self) -> Response {
        Negotiate(self).into_response()
    }
}
