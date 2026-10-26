//! AIP-160 filter to a SQL WHERE condition, values written into the SQL text.
//!
//! `capacity > 5`              → `capacity > 5`
//! `id = "9d4d…"`              → `id = '9d4d…'`
//! `staff_ids:42`              → `42 = ANY(staff_ids)` (an array column; `:` on a column that is not a list is a 400)
//! `sea_facing = true` (no such column) → `attributes -> 'sea_facing' = 'true'::jsonb`

use aip_160::{Comparator, Expression, Filter, Restriction, Value};
use sqlx::{Database, Postgres, QueryBuilder};

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
    let column = columns.iter().find(|column| column.name == field);
    let is_column = column.is_some();
    let operator = match comparator {
        Comparator::Equal => "=",
        Comparator::NotEqual => "<>",
        Comparator::GreaterThan => ">",
        Comparator::GreaterThanOrEqual => ">=",
        Comparator::LessThan => "<",
        Comparator::LessThanOrEqual => "<=",
        Comparator::Has if let Some(column) = column => return Postgres::has(column, value),
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

/// A database's SQL for `column:value`, membership of a list column. A column that is neither an array nor JSON has no
/// members.
pub trait Has: Database {
    fn has(column: &ColumnMeta, value: &Value) -> Result<String, ApiError>;
}

/// An array is a native array; JSON is `jsonb`.
impl Has for Postgres {
    fn has(column: &ColumnMeta, value: &Value) -> Result<String, ApiError> {
        match (column.is_array, column.is_json) {
            (true, _) => Ok(format!("{} = ANY({})", literal(value), column.name)),
            (false, true) => Ok(format!("{} @> {}::jsonb", column.name, quote(&format!("[{}]", json(value))))),
            (false, false) => Err(not_a_list(column)),
        }
    }
}

/// An array and JSON are both JSON text.
#[cfg(feature = "sqlite")]
impl Has for sqlx::Sqlite {
    fn has(column: &ColumnMeta, value: &Value) -> Result<String, ApiError> {
        (column.is_array || column.is_json)
            .then(|| format!("EXISTS (SELECT 1 FROM json_each({}) WHERE value = {})", column.name, literal(value)))
            .ok_or_else(|| not_a_list(column))
    }
}

fn not_a_list(column: &ColumnMeta) -> ApiError {
    ApiError::BadRequest(format!("{} is not a list", column.name))
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
