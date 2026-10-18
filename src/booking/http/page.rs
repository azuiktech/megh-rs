//! Lists as AIP-132, AIP-158 and AIP-160 describe them: `filter`, `page_size`, `page_token`, `show_deleted`,
//! and a response of the items with a `next_page_token`.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;
use serde_json::{json, Value};

use super::error::ApiError;
use crate::booking::{Attributes, Filter, Predicate};

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 1000;

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub page_size: Option<i64>,
    pub page_token: Option<String>,
    pub filter: Option<String>,
    pub show_deleted: Option<bool>,
}

fn offset_of(token: Option<&str>) -> Result<usize, ApiError> {
    let invalid = || ApiError::invalid("page_token is not valid");
    match token.filter(|token| !token.is_empty()) {
        None => Ok(0),
        Some(token) => {
            let bytes = URL_SAFE_NO_PAD.decode(token).map_err(|_| invalid())?;
            String::from_utf8(bytes).ok().and_then(|text| text.parse().ok()).ok_or_else(invalid)
        }
    }
}

fn size_of(requested: Option<i64>) -> Result<usize, ApiError> {
    match requested {
        None | Some(0) => Ok(DEFAULT_PAGE_SIZE),
        Some(size) if size < 0 => Err(ApiError::invalid("page_size cannot be negative")),
        Some(size) => Ok((size as usize).min(MAX_PAGE_SIZE)),
    }
}

/// The items that satisfy the AIP-160 `filter`, judged on their JSON fields.
fn filtered(items: Vec<Value>, filter: Option<&str>) -> Result<Vec<Value>, ApiError> {
    let Some(expression) = filter else { return Ok(items) };
    let filter = Filter::parse(expression)?;
    let holds = |item: &Value| item.as_object().is_some_and(|fields| filter.holds(&fields.iter().map(|(key, value)| (key.clone(), value.clone())).collect::<Attributes>()));
    Ok(items.into_iter().filter(holds).collect())
}

/// `{"<collection>": [...], "next_page_token": "..."}` for one page of the items.
pub fn page(collection: &str, items: Vec<Value>, query: &ListQuery) -> Result<Value, ApiError> {
    let (offset, size) = (offset_of(query.page_token.as_deref())?, size_of(query.page_size)?);
    let items = filtered(items, query.filter.as_deref())?;
    let next = (offset + size < items.len()).then(|| URL_SAFE_NO_PAD.encode((offset + size).to_string())).unwrap_or_default();
    let shown: Vec<_> = items.into_iter().skip(offset).take(size).collect();
    Ok(json!({ collection: shown, "next_page_token": next }))
}
