//! AIP-160 filter to a SQL WHERE condition, values written into the SQL text.
//!
//! `capacity > 5`              → `capacity > 5`
//! `id = "9d4d…"`              → `id = '9d4d…'`
//! `staff_ids:42`              → `42 = ANY(staff_ids)`
//! `sea_facing = true` (no such column) → `attributes -> 'sea_facing' = 'true'::jsonb`

use aip_160::{Comparator, Expression, Filter, Restriction, Value};
use sqlx::{Postgres, QueryBuilder};

use super::rest::ApiError;
use crate::entity::ColumnMeta;

/// Appends ` AND (<condition>)`.
pub fn push_filter(query: &mut QueryBuilder<'_, Postgres>, columns: &[ColumnMeta], filter: &Filter) -> Result<(), ApiError> {
    query.push(format!(" AND ({})", condition(columns, &filter.expression)?));
    Ok(())
}

fn condition(columns: &[ColumnMeta], expression: &Expression) -> Result<String, ApiError> {
    Ok(match expression {
        Expression::And(left, right) => format!("({} AND {})", condition(columns, left)?, condition(columns, right)?),
        Expression::Or(left, right) => format!("({} OR {})", condition(columns, left)?, condition(columns, right)?),
        Expression::Not(inner) => format!("NOT ({})", condition(columns, inner)?),
        Expression::Restriction(restriction) => restriction_sql(columns, restriction)?,
        Expression::Sequence(_) => return Err(ApiError::BadRequest("each filter term needs a field and a value".into())),
    })
}

fn restriction_sql(columns: &[ColumnMeta], restriction: &Restriction) -> Result<String, ApiError> {
    let Restriction { field, comparator, value } = restriction;
    let is_column = columns.iter().any(|column| column.name == field);
    let operator = match comparator {
        Comparator::Equal => "=",
        Comparator::NotEqual => "<>",
        Comparator::GreaterThan => ">",
        Comparator::GreaterThanOrEqual => ">=",
        Comparator::LessThan => "<",
        Comparator::LessThanOrEqual => "<=",
        Comparator::Has if is_column => return Ok(format!("{} = ANY({field})", literal(value))),
        Comparator::Has => return Ok(format!("attributes -> {} @> {}::jsonb", quote(field), quote(&format!("[{}]", json(value))))),
    };
    Ok(match (is_column, value) {
        (true, Value::Null) if operator == "=" => format!("{field} IS NULL"),
        (true, Value::Null) if operator == "<>" => format!("{field} IS NOT NULL"),
        (true, Value::Null) => return Err(ApiError::BadRequest(format!("{field} {operator} null is not a comparison"))),
        (true, _) => format!("{field} {operator} {}", literal(value)),
        (false, _) => format!(
            "(jsonb_typeof(attributes -> {key}) = {kind} AND attributes -> {key} {operator} {}::jsonb)",
            quote(&json(value)),
            key = quote(field),
            kind = quote(json_kind(value)),
        ),
    })
}

/// A value as SQL: text and ids in single quotes, a `'` inside written as `''`.
fn literal(value: &Value) -> String {
    match value {
        Value::String(text) => quote(text),
        Value::Number(number) => number.to_string(),
        Value::Boolean(flag) => flag.to_string(),
        Value::Null => "NULL".into(),
    }
}

fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

fn json(value: &Value) -> String {
    match value {
        Value::String(text) => serde_json::Value::from(text.as_str()).to_string(),
        other => literal(other).to_lowercase(),
    }
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Boolean(_) => "boolean",
        Value::Null => "null",
    }
}
