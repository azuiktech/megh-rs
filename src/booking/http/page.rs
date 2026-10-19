//! Lists as AIP-132, AIP-158 and AIP-160 describe them: `filter`, `page_size`, `page_token`, `show_deleted`,
//! and a response of the items with a `next_page_token`.

use std::cmp::Ordering;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::DateTime;
use base64::Engine;
use serde::Serialize;
use serde_json::Value;

use crate::auth::api::{ApiError, ListQuery, Paginated};
use crate::booking::{Attributes, Filter, Predicate};

const DEFAULT_PAGE_SIZE: usize = 50;
const MAX_PAGE_SIZE: usize = 1000;

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
fn filtered<T>(items: Vec<(Value, T)>, filter: Option<&str>) -> Result<Vec<(Value, T)>, ApiError> {
    let Some(expression) = filter else { return Ok(items) };
    let filter = Filter::parse(expression)?;
    let holds = |item: &Value| item.as_object().is_some_and(|fields| filter.holds(&fields.iter().map(|(key, value)| (key.clone(), value.clone())).collect::<Attributes>()));
    Ok(items.into_iter().filter(|(fields, _)| holds(fields)).collect())
}

/// The fields of an `order_by` such as `capacity desc, display_name`, each with whether it descends.
fn orders(text: &str) -> Result<Vec<(String, bool)>, ApiError> {
    let invalid = || ApiError::invalid("order_by is a list of fields, each with an optional desc, such as `capacity desc, display_name`");
    text.split(',').map(|part| match part.split_whitespace().collect::<Vec<_>>()[..] {
        [field] => Ok((field.to_string(), false)),
        [field, direction] if direction.eq_ignore_ascii_case("desc") => Ok((field.to_string(), true)),
        [field, direction] if direction.eq_ignore_ascii_case("asc") => Ok((field.to_string(), false)),
        _ => Err(invalid()),
    }).collect()
}

fn rank(value: &Value) -> u8 {
    match value {
        Value::Null => 0,
        Value::Bool(_) => 1,
        Value::Number(_) => 2,
        Value::String(_) => 3,
        _ => 4,
    }
}

/// Nothing first, then booleans, numbers (as numbers), strings (times as times, otherwise as text).
fn compare(left: &Value, right: &Value) -> Ordering {
    match (left, right) {
        (Value::Bool(a), Value::Bool(b)) => a.cmp(b),
        (Value::Number(a), Value::Number(b)) => a.as_f64().partial_cmp(&b.as_f64()).unwrap_or(Ordering::Equal),
        (Value::String(a), Value::String(b)) => match (DateTime::parse_from_rfc3339(a), DateTime::parse_from_rfc3339(b)) {
            (Ok(a), Ok(b)) => a.cmp(&b),
            _ => a.cmp(b),
        },
        (a, b) if rank(a) == rank(b) && rank(a) == 4 => a.to_string().cmp(&b.to_string()),
        (a, b) => rank(a).cmp(&rank(b)),
    }
}

/// The items ordered by the fields of `order_by` (the order they came in when it is absent).
fn ordered<T>(mut items: Vec<(Value, T)>, order_by: Option<&str>) -> Result<Vec<(Value, T)>, ApiError> {
    let Some(text) = order_by.filter(|text| !text.trim().is_empty()) else { return Ok(items) };
    let fields = orders(text)?;
    if let Some(unknown) = fields.iter().find(|(field, _)| !items.is_empty() && items.iter().all(|(item, _)| item.get(field).is_none())) {
        return Err(ApiError::invalid(format!("cannot order by {}: no such field", unknown.0)));
    }
    items.sort_by(|(left, _), (right, _)| {
        fields.iter().map(|(field, descending)| {
            let ordering = compare(left.get(field).unwrap_or(&Value::Null), right.get(field).unwrap_or(&Value::Null));
            if *descending { ordering.reverse() } else { ordering }
        }).find(|ordering| ordering.is_ne()).unwrap_or(Ordering::Equal)
    });
    Ok(items)
}

/// The page of the items that satisfy the query's filter, in the query's order.
pub fn paginate<T: Serialize>(items: Vec<T>, query: &ListQuery) -> Result<Paginated<T>, ApiError> {
    let (offset, size) = (offset_of(query.page_token.as_deref())?, size_of(query.page_size)?);
    let fields = items.into_iter().map(|item| Ok((serde_json::to_value(&item).map_err(|error| ApiError::invalid(error.to_string()))?, item))).collect::<Result<Vec<_>, ApiError>>()?;
    let items = ordered(filtered(fields, query.filter.as_deref())?, query.order_by.as_deref())?;
    let next = (offset + size < items.len()).then(|| URL_SAFE_NO_PAD.encode((offset + size).to_string())).unwrap_or_default();
    Ok(Paginated { data: items.into_iter().skip(offset).take(size).map(|(_, item)| item).collect(), next_page_token: next })
}
