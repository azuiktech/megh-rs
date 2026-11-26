//! Shared REST layer: app context, one typed input per route, errors, pages.

use std::collections::HashMap;

use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum_params::Params;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

#[derive(Clone)]
pub struct AppContext {
    pub pool: crate::Db,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::NotFound | Self::Database(sqlx::Error::RowNotFound) => StatusCode::NOT_FOUND,
            Self::Database(sqlx::Error::Database(error)) if error.is_foreign_key_violation() => StatusCode::BAD_REQUEST,
            Self::Database(sqlx::Error::Database(error)) if error.is_unique_violation() => StatusCode::CONFLICT,
            Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
        };
        (status, self.to_string()).into_response()
    }
}

/// Path, query and body as one `T`, checked by its `validate()`; path values overwrite the others.
pub struct Input<T>(pub T);

impl<T: DeserializeOwned + Validate, S: Send + Sync> FromRequest<S> for Input<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let bad = |e: &dyn std::fmt::Debug| ApiError::BadRequest(format!("{e:?}"));
        let (mut parts, body) = req.into_parts();
        let Path(path) = Path::<HashMap<String, String>>::from_request_parts(&mut parts, state).await.map_err(|e| bad(&e))?;
        let Params(serde_json::Value::Object(mut fields), _) = Params::<serde_json::Value>::from_request(Request::from_parts(parts, body), state).await.map_err(|e| bad(&e))? else {
            return Err(ApiError::BadRequest("expected an object".into()));
        };
        fields.extend(path.into_iter().map(|(name, value)| (name, serde_json::Value::String(value))));
        let value: T = serde_json::from_value(fields.into()).map_err(|e| bad(&e))?;
        value.validate().map_err(|e| ApiError::BadRequest(e.to_string()))?;
        Ok(Input(value))
    }
}

/// A list request: the tenant from the path, an AIP-160 filter, and the page.
#[derive(Debug, Deserialize, Validate)]
pub struct ListRequest {
    pub tenant_id: Uuid,
    #[serde(default, deserialize_with = "parse_filter")]
    pub filter: Option<aip_160::Filter>,
    #[serde(flatten)]
    pub page: PageRequest,
}

/// Cursor paging (AIP-158); `page_token` is a cursor from `paginator-utils`.
#[serde_with::serde_as]
#[derive(Debug, Deserialize, utoipa::ToSchema)]
pub struct PageRequest {
    #[serde_as(as = "Option<serde_with::PickFirst<(_, serde_with::DisplayFromStr)>>")]
    #[serde(default)]
    pub page_size: Option<u16>,
    pub page_token: Option<String>,
}

impl PageRequest {
    /// The page size asked for, 50 when none is, at most 1000.
    pub fn size(&self) -> u16 {
        self.page_size.unwrap_or(PAGE_SIZE).clamp(1, MAX_PAGE_SIZE)
    }
}

#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page_token: Option<String>,
}

impl<T: Serialize> IntoResponse for Page<T> {
    fn into_response(self) -> Response {
        axum::Json(self).into_response()
    }
}

fn parse_filter<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<aip_160::Filter>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    aip_160::parse_filter(&text).map(Some).map_err(serde::de::Error::custom)
}

/// One row of a tenant by its key: `/tenants/{tenant_id}/<collection>/{id}`.
#[derive(Debug, Deserialize, Validate)]
pub struct Key {
    pub tenant_id: Uuid,
    pub id: Uuid,
}

/// `(tenant_id, id)`.
impl From<(Uuid, Uuid)> for Key {
    fn from((tenant_id, id): (Uuid, Uuid)) -> Self {
        Key { tenant_id, id }
    }
}

/// A PATCH body for the row named by the path: the JSON merge patch (RFC 7396) to apply.
#[derive(Debug, Deserialize, Validate)]
pub struct Changes {
    #[serde(flatten)]
    pub key: Key,
    #[serde(flatten)]
    pub changes: serde_json::Map<String, serde_json::Value>,
}

const PAGE_SIZE: u16 = 50;
const MAX_PAGE_SIZE: u16 = 1000;
