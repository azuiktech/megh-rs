//! AIP-160 filter to a `sea_query::Condition`, the same on every database.
//!
//! `capacity > 5`          → `capacity > $1`, the value converted by the column's own type
//! `sea_facing = true`     → `attributes -> 'sea_facing' = 'true'` (no such column: the JSON value of `attributes`)

use aip_160::{Comparator, Expression, Filter, Restriction, Value as Literal};
use sea_query::{Alias, BinOper, Condition, Expr, ExprTrait, SimpleExpr, Value};

use crate::auth::rest::{ApiError, ApiResult};
use crate::auth::column::{Elem, Kind};
use crate::auth::db::Backend;
use crate::auth::entity::{ColumnMeta, Table};

pub fn condition<T: Table, DB: Backend>(filter: &Filter) -> ApiResult<Condition> {
    expression::<T, DB>(&filter.expression)
}

fn expression<T: Table, DB: Backend>(expression: &Expression) -> ApiResult<Condition> {
    match expression {
        Expression::And(left, right) => Ok(Condition::all().add(self::expression::<T, DB>(left)?).add(self::expression::<T, DB>(right)?)),
        Expression::Or(left, right) => Ok(Condition::any().add(self::expression::<T, DB>(left)?).add(self::expression::<T, DB>(right)?)),
        Expression::Not(inner) => Ok(Condition::all().not().add(self::expression::<T, DB>(inner)?)),
        Expression::Restriction(restriction) => restriction_of::<T, DB>(restriction),
        Expression::Sequence(_) => Err(ApiError::BadRequest("each filter term needs a field and a value".into())),
    }
}

fn restriction_of<T: Table, DB: Backend>(restriction: &Restriction) -> ApiResult<Condition> {
    let Restriction { field, comparator, value } = restriction;
    let json = json(value);
    match comparator {
        Comparator::Has => Ok(Condition::all().add(has::<T, DB>(field, &json)?)),
        _ => compare::<T>(restriction, &json),
    }
}

fn compare<T: Table>(Restriction { field, comparator, value }: &Restriction, json: &serde_json::Value) -> ApiResult<Condition> {
    let (left, right) = match T::COLUMNS.iter().find(|column| column.name == field.as_str()) {
        Some(column) => (Expr::col(Alias::new(column.name)).into(), T::from_json(column.name, json).map(|value| column.expr(value))),
        None => (attribute(field), Ok(Expr::val(Value::Json(Some(Box::new(json.clone())))).into())),
    };
    let right: SimpleExpr = right.map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let condition = match (comparator, value) {
        (Comparator::Equal, Literal::Null) => left.is_null(),
        (Comparator::NotEqual, Literal::Null) => left.is_not_null(),
        (comparator, _) => left.binary(operator(comparator, field)?, right),
    };
    Ok(Condition::all().add(condition))
}

/// `field:value`: the value is an element of a list column, or of a JSON array under `attributes`.
fn has<T: Table, DB: Backend>(field: &str, json: &serde_json::Value) -> ApiResult<SimpleExpr> {
    let bad = |message: String| ApiError::BadRequest(message);
    match T::COLUMNS.iter().find(|column| column.name == field) {
        Some(ColumnMeta { kind: Kind::Array(elem), name, .. }) => {
            let value = element(*elem, json).ok_or_else(|| bad(format!("{json} is not an element of {field}")))?;
            Ok(DB::has(Expr::col(Alias::new(*name)).into(), value))
        }
        Some(_) => Err(bad(format!("{field} is not a list"))),
        None => Ok(DB::has_json(attribute(field), element_of(json).ok_or_else(|| bad(format!("{json} cannot be searched for")))?)),
    }
}

fn element(elem: Elem, json: &serde_json::Value) -> Option<Value> {
    match (elem, json) {
        (Elem::Uuid, serde_json::Value::String(text)) => text.parse::<uuid::Uuid>().ok().map(Value::from),
        (Elem::Text, serde_json::Value::String(text)) => Some(Value::from(text.clone())),
        (Elem::Int, serde_json::Value::Number(number)) => number.as_i64().map(Value::from),
        _ => None,
    }
}

fn element_of(json: &serde_json::Value) -> Option<Value> {
    match json {
        serde_json::Value::String(text) => Some(Value::from(text.clone())),
        serde_json::Value::Number(number) => number.as_i64().map(Value::from).or_else(|| number.as_f64().map(Value::from)),
        serde_json::Value::Bool(flag) => Some(Value::from(*flag)),
        _ => None,
    }
}

/// The JSON value under `key` in `attributes`; the key is a bound parameter, never part of the SQL text.
fn attribute(key: &str) -> SimpleExpr {
    Expr::col(Alias::new("attributes")).binary(BinOper::Custom("->"), Expr::val(key.to_string()))
}

fn operator(comparator: &Comparator, field: &str) -> ApiResult<BinOper> {
    match comparator {
        Comparator::Equal => Ok(BinOper::Equal),
        Comparator::NotEqual => Ok(BinOper::NotEqual),
        Comparator::GreaterThan => Ok(BinOper::GreaterThan),
        Comparator::GreaterThanOrEqual => Ok(BinOper::GreaterThanOrEqual),
        Comparator::LessThan => Ok(BinOper::SmallerThan),
        Comparator::LessThanOrEqual => Ok(BinOper::SmallerThanOrEqual),
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
