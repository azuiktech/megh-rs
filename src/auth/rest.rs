//! Shared REST layer: app context, one typed input per route, errors, pages.

use std::collections::HashMap;

use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum_params::{Params, Value};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone)]
pub struct AppContext {
    pub pool: sqlx::PgPool,
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::NotFound | Self::Database(sqlx::Error::RowNotFound) => StatusCode::NOT_FOUND,
            Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
        };
        (status, self.to_string()).into_response()
    }
}

/// Path, query and body as one `T`; path values overwrite the others.
pub struct Input<T>(pub T);

impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for Input<T> {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, ApiError> {
        let bad = |e: &dyn std::fmt::Debug| ApiError::BadRequest(format!("{e:?}"));
        let (mut parts, body) = req.into_parts();
        let Path(path) = Path::<HashMap<String, String>>::from_request_parts(&mut parts, state).await.map_err(|e| bad(&e))?;
        let Params(Value::Object(mut fields), _) = Params::<Value>::from_request(Request::from_parts(parts, body), state).await.map_err(|e| bad(&e))? else {
            return Err(ApiError::BadRequest("expected an object".into()));
        };
        fields.extend(path.into_iter().map(|(name, value)| (name, Value::XStr(value))));
        T::deserialize(Value::Object(fields)).map(Input).map_err(|e| bad(&e))
    }
}

/// A list request: the org from the path, an AIP-160 filter, and the page.
#[derive(Debug, Deserialize)]
pub struct ListRequest {
    pub org_id: Uuid,
    #[serde(default, deserialize_with = "parse_filter")]
    pub filter: Option<aip_160::Filter>,
    #[serde(flatten)]
    pub page: PageRequest,
}

/// Cursor paging (AIP-158); `page_token` is a cursor from `paginator-utils`.
#[derive(Debug, Deserialize)]
pub struct PageRequest {
    pub page_size: Option<u16>,
    pub page_token: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_page_token: Option<String>,
}

fn parse_filter<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<aip_160::Filter>, D::Error> {
    let text = String::deserialize(deserializer)?;
    if text.trim().is_empty() {
        return Ok(None);
    }
    aip_160::parse_filter(&text).map(Some).map_err(serde::de::Error::custom)
}
