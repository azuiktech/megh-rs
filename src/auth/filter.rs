//! AIP-160 filter to a `Where`, on every driver.
//!
//! `capacity > 5`          → `capacity > ?`, the value bound as the field's own type
//! `sea_facing = true`     → the JSON value under `sea_facing` in `attributes` (no such column)
//! `service_ids:<id>`      → the value is an element of a list column, or of a JSON array under `attributes`

use aip_160::{Comparator, Expression, Filter, Restriction, Value as Literal};

use crate::auth::entity::{Table, Where};
use crate::auth::rest::{ApiError, ApiResult};

pub fn condition<T: Table>(filter: &Filter) -> ApiResult<Where> {
    expression::<T>(&filter.expression)
}

fn expression<T: Table>(expression: &Expression) -> ApiResult<Where> {
    match expression {
        Expression::And(left, right) => Ok(self::expression::<T>(left)?.and(self::expression::<T>(right)?)),
        Expression::Or(left, right) => Ok(self::expression::<T>(left)?.or(self::expression::<T>(right)?)),
        Expression::Not(inner) => Ok(self::expression::<T>(inner)?.not()),
        Expression::Restriction(restriction) => restriction_of::<T>(restriction),
        Expression::Sequence(_) => Err(ApiError::BadRequest("each filter term needs a field and a value".into())),
    }
}

fn restriction_of<T: Table>(restriction: &Restriction) -> ApiResult<Where> {
    let Restriction { field, comparator, value } = restriction;
    let json = json(value);
    match (comparator, value, T::COLUMNS.contains(&field.as_str())) {
        (Comparator::Has, _, true) => has::<T>(field, &json),
        (Comparator::Has, _, false) => Ok(has_attribute(field, &json)),
        (Comparator::Equal, Literal::Null, true) => Ok(Where::new(format!("\"{field}\" IS NULL"), vec![])),
        (Comparator::NotEqual, Literal::Null, true) => Ok(Where::new(format!("\"{field}\" IS NOT NULL"), vec![])),
        (_, Literal::Number(_), true) => {
            let (value, operator) = (column_value::<T>(field, &json)?, operator(comparator, field)?);
            Ok(Where::new(format!("\"{field}\" {operator} ?"), vec![value]).sqlite(format!("CAST(\"{field}\" AS NUMERIC) {operator} CAST(? AS NUMERIC)")))
        }
        (_, _, true) => Ok(Where::new(format!("\"{field}\" {} ?", operator(comparator, field)?), vec![column_value::<T>(field, &json)?])),
        (_, _, false) => compare_attribute(field, comparator, &json),
    }
}

/// `json` as the column takes it, through the field's own type.
fn column_value<T: Table>(field: &str, json: &serde_json::Value) -> ApiResult<crate::auth::db::Value> {
    T::param(field, json).map_err(|error| ApiError::BadRequest(error.to_string()))
}

/// The JSON value under `key` in `attributes` compared with `json`; the key is a bound parameter, never SQL text.
fn compare_attribute(key: &str, comparator: &Comparator, json: &serde_json::Value) -> ApiResult<Where> {
    let operator = operator(comparator, key)?;
    Ok(Where::new(format!("json_extract(attributes, '$.' || ?) {operator} json_extract(?, '$')"), vec![Box::new(key.to_owned()), Box::new(json.to_string())])
        .postgres(format!("attributes -> ? {operator} CAST(? AS jsonb)")))
}

/// `field:value` on a column: the value is an element of the list the column holds.
fn has<T: Table>(field: &str, json: &serde_json::Value) -> ApiResult<Where> {
    T::LISTS.contains(&field).then_some(()).ok_or_else(|| ApiError::BadRequest(format!("{field} is not a list")))?;
    let element = element(json).ok_or_else(|| ApiError::BadRequest(format!("{json} is not an element of {field}")))?;
    Ok(Where::new(format!("EXISTS (SELECT 1 FROM json_each(\"{field}\") WHERE CAST(value AS TEXT) = ?)"), vec![Box::new(element)])
        .postgres(format!("CAST(? AS text) = ANY(CAST(\"{field}\" AS text[]))")))
}

/// `key:value` on `attributes`: the value is an element of the JSON array under `key`.
fn has_attribute(key: &str, json: &serde_json::Value) -> Where {
    let array = serde_json::Value::Array(vec![json.clone()]).to_string();
    Where::new("EXISTS (SELECT 1 FROM json_each(attributes, '$.' || ?) WHERE value = json_extract(?, '$[0]'))", vec![Box::new(key.to_owned()), Box::new(array)])
        .postgres("attributes -> ? @> CAST(? AS jsonb)")
}

/// A list element as text, the way both drivers compare it.
fn element(json: &serde_json::Value) -> Option<String> {
    match json {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn operator(comparator: &Comparator, field: &str) -> ApiResult<&'static str> {
    match comparator {
        Comparator::Equal => Ok("="),
        Comparator::NotEqual => Ok("<>"),
        Comparator::GreaterThan => Ok(">"),
        Comparator::GreaterThanOrEqual => Ok(">="),
        Comparator::LessThan => Ok("<"),
        Comparator::LessThanOrEqual => Ok("<="),
        Comparator::Has => Err(ApiError::BadRequest(format!("{field}: `:` is not supported"))),
    }
}

fn json(value: &Literal) -> serde_json::Value {
    match value {
        Literal::String(text) => text.clone().into(),
        Literal::Number(number) => number.to_string().parse().unwrap_or(serde_json::Value::Null),
        Literal::Boolean(flag) => (*flag).into(),
        Literal::Null => serde_json::Value::Null,
    }
}
