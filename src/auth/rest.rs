//! Shared REST layer: app context, one typed input per route, errors, pages.

use std::collections::HashMap;

use axum::extract::{FromRequest, FromRequestParts, Path, Request};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum_params::Params;
use paginator_utils::{Cursor, CursorDirection, CursorValue};
use sqlx::postgres::PgRow;
use sqlx::{FromRow, PgPool, Postgres, QueryBuilder};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use validator::Validate;

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
#[derive(Debug, Deserialize)]
pub struct PageRequest {
    #[serde_as(as = "Option<serde_with::PickFirst<(_, serde_with::DisplayFromStr)>>")]
    #[serde(default)]
    pub page_size: Option<u16>,
    pub page_token: Option<String>,
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

/// A body for the row named by the path: the entity's data for PUT, the changes for PATCH.
#[derive(Debug, Deserialize, Validate)]
pub struct Keyed<T: Validate> {
    pub id: Uuid,
    #[serde(flatten)]
    #[validate(nested)]
    pub data: T,
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

/// Runs `query` (a SELECT whose rows have an `id`) one page at a time, ordered by `id`.
pub async fn fetch_page<T>(pool: &PgPool, mut query: QueryBuilder<'_, Postgres>, page: &PageRequest) -> ApiResult<Page<T>>
where
    T: for<'r> FromRow<'r, PgRow> + Serialize + Send + Unpin,
{
    let size = page.page_size.unwrap_or(PAGE_SIZE).clamp(1, MAX_PAGE_SIZE);
    if let Some(token) = &page.page_token {
        let bad_token = || ApiError::BadRequest("invalid page_token".into());
        let cursor = Cursor::decode(token).map_err(|_| bad_token())?;
        let CursorValue::Uuid(after) = cursor.value else { return Err(bad_token()) };
        let after: Uuid = after.parse().map_err(|_| bad_token())?;
        query.push(" AND id > ").push_bind(after);
    }
    query.push(" ORDER BY id LIMIT ").push_bind(i64::from(size) + 1);
    let mut items: Vec<T> = query.build_query_as().fetch_all(pool).await?;
    let has_more = items.len() > usize::from(size);
    items.truncate(usize::from(size));
    let next_page_token = match (has_more, items.last()) {
        (true, Some(last)) => Some(Cursor::from_row("id", last, CursorDirection::After).and_then(|cursor| cursor.encode()).map_err(ApiError::BadRequest)?),
        _ => None,
    };
    Ok(Page { items, next_page_token })
}
