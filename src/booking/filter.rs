//! A condition written as a Google AIP-160 filter expression, such as `role = "stylist" AND experience_years >= 10`,
//! evaluated against an entity's key/value attributes. The `aip-filtering` crate parses the grammar.
//!
//! Supported: `AND`, `OR` (binding tighter, as AIP-160 says), `NOT` and `-`, implicit `AND` between restrictions,
//! `=` `!=` `<` `<=` `>` `>=` on strings, numbers, booleans and timestamps, `*` wildcards in string equality,
//! `.` traversal into nested objects, and `:` on arrays (any element), objects (key present) and scalars (equal).
//! A field that is missing makes its restriction false, `!=` included.
//!
//! Rejected as `Invalid`, because the parser (`aip-filtering` 0.1) mis-reads them as plain words and a mis-read
//! condition must not pass silently: functions, a comparator without spaces around it (`role="stylist"`), a
//! missing right-hand side (`a =`), and decimal numbers (`4.5` reads as a traversal). Write `role = "stylist"` and integers.

use std::cmp::Ordering;
use std::panic::{catch_unwind, AssertUnwindSafe};

use aip_filtering::ast::{BinOp, Comparator, Expression, FilterParser, UnOp, Value as Literal};
use chrono::{DateTime, FixedOffset};
use itertools::Itertools;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{Attributes, Error, Predicate, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String")]
pub struct Filter(String);

/// A filter is checked when it is read, so a stored or posted condition is one the engine can evaluate.
impl TryFrom<String> for Filter {
    type Error = Error;

    fn try_from(text: String) -> Result<Self> {
        Self::parse(text)
    }
}

/// The parser is young and unwraps inside; whatever it does on odd input must not take the caller down.
/// An empty text is no filter at all.
fn parsed<T>(text: &str, then: impl FnOnce(Option<&Expression>) -> T) -> Result<T> {
    if text.trim().is_empty() {
        return Ok(then(None));
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| FilterParser::parse_str(text).map(|filter| then(filter.as_ref())).map_err(|error| error.to_string())));
    outcome.map_err(|_| Error::Invalid("filter: cannot be parsed".into()))?.map_err(|error| Error::Invalid(format!("filter: {error}")))
}

fn unsupported(reason: &str) -> Result<()> {
    Err(Error::Invalid(format!("filter: {reason}")))
}

/// An unquoted word must not hide a comparator or a quote the parser failed to split off.
fn word(literal: &Literal) -> Result<()> {
    match literal {
        Literal::Text(text) if text.contains(['=', '<', '>', '!', '"', '\'']) => unsupported("put a space around the comparator, as in role = \"stylist\", and close every quote"),
        _ => Ok(()),
    }
}

fn numeric(literal: &Literal) -> bool {
    matches!(literal, Literal::Int(_) | Literal::Float(_))
}

fn check(expression: &Expression) -> Result<()> {
    match expression {
        Expression::FCall { .. } => unsupported("functions are not supported"),
        Expression::Binary { lhs, rhs, .. } => check(lhs).and(check(rhs)),
        Expression::Unary { rhs, .. } => check(rhs),
        Expression::Sequence(items) => items.iter().try_for_each(check),
        Expression::Restriction { lhs, rhs, .. } => match rhs.as_ref() {
            Expression::Value(literal) => check(lhs).and(word(literal)),
            Expression::Member { value, path } if numeric(value) && path.iter().all(numeric) => unsupported("decimal numbers are not supported"),
            _ => unsupported("the right side of a comparison must be a single value"),
        },
        Expression::Value(literal) => word(literal),
        Expression::Member { value, path } => word(value).and(path.iter().try_for_each(word)),
    }
}

impl Filter {
    /// The filter, or `Invalid` when the text is not an AIP-160 expression this module can evaluate.
    pub fn parse(text: impl Into<String>) -> Result<Self> {
        let text = text.into();
        parsed(&text, |expression| expression.map_or(Ok(()), check))??;
        Ok(Self(text))
    }

    pub fn expression(&self) -> &str {
        &self.0
    }
}

#[typetag::serde]
impl Predicate for Filter {
    fn holds(&self, subject: &Attributes) -> bool {
        parsed(&self.0, |expression| expression.is_none_or(|expression| evaluate(expression, subject))).unwrap_or(false)
    }
}

fn evaluate(expression: &Expression, subject: &Attributes) -> bool {
    match expression {
        Expression::Binary { lhs, op: BinOp::And, rhs } => evaluate(lhs, subject) && evaluate(rhs, subject),
        Expression::Binary { lhs, op: BinOp::Or, rhs } => evaluate(lhs, subject) || evaluate(rhs, subject),
        Expression::Unary { op: UnOp::Not | UnOp::Neg, rhs } => !evaluate(rhs, subject),
        Expression::Sequence(items) => items.iter().all(|item| evaluate(item, subject)),
        Expression::Restriction { lhs, op, rhs } => restriction(lhs, *op, rhs, subject),
        Expression::Value(literal) => mentioned(literal, subject),
        Expression::Member { .. } | Expression::FCall { .. } => false,
    }
}

fn key(literal: &Literal) -> String {
    literal.string_repr()
}

/// The value a field names, following `.` into nested objects.
fn resolve<'a>(field: &Expression, subject: &'a Attributes) -> Option<&'a Value> {
    match field {
        Expression::Value(name) => subject.get(&key(name)),
        Expression::Member { value, path } => path.iter().try_fold(subject.get(&key(value))?, |found, step| found.get(key(step))),
        _ => None,
    }
}

fn restriction(field: &Expression, op: Comparator, wanted: &Expression, subject: &Attributes) -> bool {
    let (Some(found), Expression::Value(literal)) = (resolve(field, subject), wanted) else { return false };
    match op {
        Comparator::Eq => equals(found, literal),
        Comparator::Ne => !equals(found, literal),
        Comparator::Has => has(found, literal),
        Comparator::Lt => order(found, literal).is_some_and(Ordering::is_lt),
        Comparator::LtEq => order(found, literal).is_some_and(Ordering::is_le),
        Comparator::Gt => order(found, literal).is_some_and(Ordering::is_gt),
        Comparator::GtEq => order(found, literal).is_some_and(Ordering::is_ge),
    }
}

/// A string with `*` wildcards, and nothing else special.
fn matches_pattern(pattern: &str, text: &str) -> bool {
    let expression = format!("^{}$", pattern.split('*').map(regex::escape).join(".*"));
    Regex::new(&expression).is_ok_and(|regex| regex.is_match(text))
}

fn timestamp(found: &Value) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(found.as_str()?).ok()
}

fn equals(found: &Value, wanted: &Literal) -> bool {
    match wanted {
        Literal::Bool(wanted) => found.as_bool() == Some(*wanted),
        Literal::Int(wanted) => found.as_f64() == Some(*wanted as f64),
        Literal::Float(wanted) => found.as_f64() == Some(*wanted),
        Literal::Duration(wanted) => found.as_f64() == Some(wanted.as_secs_f64()),
        Literal::Timestamp(wanted) => timestamp(found) == Some(*wanted),
        Literal::String(pattern) | Literal::Text(pattern) => found.as_str().is_some_and(|text| matches_pattern(pattern, text)),
    }
}

fn order(found: &Value, wanted: &Literal) -> Option<Ordering> {
    match wanted {
        Literal::Int(wanted) => found.as_f64()?.partial_cmp(&(*wanted as f64)),
        Literal::Float(wanted) => found.as_f64()?.partial_cmp(wanted),
        Literal::Duration(wanted) => found.as_f64()?.partial_cmp(&wanted.as_secs_f64()),
        Literal::Timestamp(wanted) => Some(timestamp(found)?.cmp(wanted)),
        Literal::String(wanted) | Literal::Text(wanted) => Some(found.as_str()?.cmp(*wanted)),
        Literal::Bool(_) => None,
    }
}

fn has(found: &Value, wanted: &Literal) -> bool {
    match found {
        Value::Array(items) => items.iter().any(|item| equals(item, wanted)),
        Value::Object(fields) => matches!(wanted, Literal::Text("*")) && !fields.is_empty() || fields.contains_key(&key(wanted)),
        scalar => equals(scalar, wanted),
    }
}

/// A bare value matches anywhere in the attributes.
fn mentioned(wanted: &Literal, subject: &Attributes) -> bool {
    let anywhere = |value: &Value| match value {
        Value::Array(items) => items.iter().any(|item| equals(item, wanted)),
        scalar => equals(scalar, wanted),
    };
    subject.as_object().is_some_and(|fields| fields.values().any(anywhere))
}
