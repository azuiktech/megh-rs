//! What differs between databases, and nothing else: a `Backend` is a query builder, a few converters (how a list or a
//! decimal is held) and the time of a write. The statements and the CRUD are the same for all of them.

use std::future::Future;

use chrono::Utc;
use sea_query::{Expr, PostgresQueryBuilder, SimpleExpr, Value, Values};
#[cfg(feature = "sqlite")]
use sea_query::SqliteQueryBuilder;
use sea_query_binder::{SqlxBinder, SqlxValues};
use sqlx::types::Json;
use sqlx::{Database, Postgres, Row};
#[cfg(feature = "sqlite")]
use sqlx::Sqlite;
use uuid::Uuid;

use crate::auth::column::{Elem, Kind};
use crate::auth::entity::ColumnMeta;

pub type Statement = (String, SqlxValues);

pub trait Backend: Database + Sized {
    fn build(statement: &impl SqlxBinder) -> Statement;
    /// The time of a write.
    fn now() -> SimpleExpr;
    /// What a patch sets a column to when it gives `null`: the column's default where the database can say so.
    fn reset() -> SimpleExpr;
    /// `value` is an element of the list column `list`.
    fn has(list: SimpleExpr, value: Value) -> SimpleExpr;
    /// `value` is an element of the JSON array `list`.
    fn has_json(list: SimpleExpr, value: Value) -> SimpleExpr;
    fn fetch(connection: &mut Self::Connection, statement: Statement, columns: &[ColumnMeta]) -> impl Future<Output = Result<Vec<Vec<Value>>, sqlx::Error>> + Send;
    fn execute(connection: &mut Self::Connection, statement: Statement) -> impl Future<Output = Result<u64, sqlx::Error>> + Send;
    /// Marks the rows a physical delete of `table` would remove through `ON DELETE CASCADE`.
    fn cascade(connection: &mut Self::Connection, table: &str) -> impl Future<Output = Result<(), sqlx::Error>> + Send;
}

/// One database: its query builder, how it holds a list (`array`, `bind_value`) and a decimal, the time of a write, and
/// the soft-delete cascade. The row reader is written once and expanded for each.
macro_rules! backend {
    ($module:ident, $db:ty, builder: $builder:ty, now: $now:expr, reset: $reset:expr, has: $has:expr, has_json: $has_json:expr, decimal: $decimal:expr, array: $array:expr, bind_value: $bind:expr, cascade: $cascade:expr) => {
        mod $module {
            use super::*;

            fn read(row: &<$db as Database>::Row, columns: &[ColumnMeta]) -> Result<Vec<Value>, sqlx::Error> {
                columns
                    .iter()
                    .map(|column| {
                        let name = column.name;
                        Ok(match column.kind {
                            Kind::Text => Value::String(row.try_get_unchecked::<Option<String>, _>(name)?.map(Box::new)),
                            Kind::Uuid => Value::Uuid(row.try_get::<Option<Uuid>, _>(name)?.map(Box::new)),
                            Kind::Date => Value::ChronoDate(row.try_get::<Option<chrono::NaiveDate>, _>(name)?.map(Box::new)),
                            Kind::Time => Value::ChronoTime(row.try_get::<Option<chrono::NaiveTime>, _>(name)?.map(Box::new)),
                            Kind::Timestamp => Value::ChronoDateTimeUtc(row.try_get::<Option<chrono::DateTime<Utc>>, _>(name)?.map(Box::new)),
                            Kind::Bool => Value::Bool(row.try_get::<Option<bool>, _>(name)?),
                            Kind::Int => Value::BigInt(match row.try_get::<Option<i64>, _>(name) {
                                Ok(number) => number,
                                Err(_) => row.try_get::<Option<i32>, _>(name)?.map(i64::from),
                            }),
                            Kind::Float => Value::Double(row.try_get::<Option<f64>, _>(name)?),
                            Kind::Decimal => Value::Decimal(($decimal)(row, name)?.map(Box::new)),
                            Kind::Bytes => Value::Bytes(row.try_get::<Option<Vec<u8>>, _>(name)?.map(Box::new)),
                            Kind::Json => Value::Json(row.try_get::<Option<Json<serde_json::Value>>, _>(name)?.map(|json| Box::new(json.0))),
                            Kind::Array(elem) => ($array)(row, name, elem)?,
                        })
                    })
                    .collect()
            }

            impl Backend for $db {
                fn build(statement: &impl SqlxBinder) -> Statement {
                    let (sql, SqlxValues(Values(values))) = statement.build_sqlx(<$builder>::default());
                    (sql, SqlxValues(Values(values.into_iter().map($bind).collect())))
                }

                fn now() -> SimpleExpr {
                    $now
                }

                fn reset() -> SimpleExpr {
                    $reset
                }

                fn has(list: SimpleExpr, value: Value) -> SimpleExpr {
                    ($has)(list, value)
                }

                fn has_json(list: SimpleExpr, value: Value) -> SimpleExpr {
                    ($has_json)(list, value)
                }

                async fn fetch(connection: &mut Self::Connection, (sql, values): Statement, columns: &[ColumnMeta]) -> Result<Vec<Vec<Value>>, sqlx::Error> {
                    let rows = sqlx::query_with(&sql, values).fetch_all(&mut *connection).await?;
                    rows.iter().map(|row| read(row, columns)).collect()
                }

                async fn execute(connection: &mut Self::Connection, (sql, values): Statement) -> Result<u64, sqlx::Error> {
                    Ok(sqlx::query_with(&sql, values).execute(&mut *connection).await?.rows_affected())
                }

                async fn cascade(connection: &mut Self::Connection, table: &str) -> Result<(), sqlx::Error> {
                    ($cascade)(connection, table).await
                }
            }
        }
    };
}

backend!(
    postgres, Postgres,
    builder: PostgresQueryBuilder,
    now: Expr::current_timestamp().into(),
    reset: Expr::cust("DEFAULT"),
    has: |list: SimpleExpr, value: Value| Expr::cust_with_exprs("$1 = ANY($2)", [Expr::val(value).into(), list]),
    has_json: |list: SimpleExpr, value: Value| Expr::cust_with_exprs("$1 @> $2", [list, Expr::val(Value::Json(Some(Box::new(serde_json::Value::Array(vec![json_of(value)]))))).into()]),
    decimal: |row: &sqlx::postgres::PgRow, name: &str| row.try_get::<Option<rust_decimal::Decimal>, _>(name),
    array: native_array,
    bind_value: |value: Value| value,
    cascade: pg_cascade
);

#[cfg(feature = "sqlite")]
backend!(
    sqlite, Sqlite,
    builder: SqliteQueryBuilder,
    now: Expr::val(Utc::now()).into(),
    reset: Expr::cust("NULL"),
    has: |list: SimpleExpr, value: Value| Expr::cust_with_exprs("EXISTS (SELECT 1 FROM json_each(?) WHERE value = ?)", [list, Expr::val(as_text(value)).into()]),
    has_json: |list: SimpleExpr, value: Value| Expr::cust_with_exprs("EXISTS (SELECT 1 FROM json_each(?) WHERE value = ?)", [list, Expr::val(as_text(value)).into()]),
    decimal: text_decimal,
    array: json_array,
    bind_value: array_as_json,
    cascade: no_cascade
);

async fn pg_cascade(connection: &mut sqlx::PgConnection, table: &str) -> Result<(), sqlx::Error> {
    crate::soft_delete::cascade(connection, table).await
}

/// Without foreign-key catalogs to read, nothing cascades.
#[cfg(feature = "sqlite")]
async fn no_cascade(_: &mut sqlx::SqliteConnection, _: &str) -> Result<(), sqlx::Error> {
    Ok(())
}

/// A value as the JSON of an array element.
fn json_of(value: Value) -> serde_json::Value {
    match value {
        Value::Uuid(Some(id)) => serde_json::Value::String(id.to_string()),
        Value::String(Some(text)) => serde_json::Value::String(*text),
        Value::BigInt(Some(number)) => number.into(),
        Value::Bool(Some(flag)) => flag.into(),
        Value::Double(Some(number)) => number.into(),
        _ => serde_json::Value::Null,
    }
}

/// An element of a JSON list as SQLite compares it: uuids are text there.
#[cfg(feature = "sqlite")]
fn as_text(value: Value) -> Value {
    match value {
        Value::Uuid(Some(id)) => Value::from(id.to_string()),
        other => other,
    }
}

/// PostgreSQL holds a list as an array.
fn native_array(row: &sqlx::postgres::PgRow, name: &str, elem: Elem) -> Result<Value, sqlx::Error> {
    let list = |items: Option<Vec<Value>>| Value::Array(elem.array_type(), items.map(Box::new));
    Ok(match elem {
        Elem::Uuid => list(row.try_get::<Option<Vec<Uuid>>, _>(name)?.map(|items| items.into_iter().map(Value::from).collect())),
        Elem::Text => list(row.try_get::<Option<Vec<String>>, _>(name)?.map(|items| items.into_iter().map(Value::from).collect())),
        Elem::Int => list(row.try_get::<Option<Vec<i64>>, _>(name)?.map(|items| items.into_iter().map(Value::from).collect())),
    })
}

#[cfg(feature = "sqlite")]
/// SQLite holds a list as JSON text.
fn json_array(row: &sqlx::sqlite::SqliteRow, name: &str, elem: Elem) -> Result<Value, sqlx::Error> {
    let decode = |error: String| sqlx::Error::Decode(error.into());
    let items = row.try_get::<Option<Json<Vec<serde_json::Value>>>, _>(name)?;
    let item = |json: serde_json::Value| match (elem, json) {
        (Elem::Uuid, serde_json::Value::String(text)) => text.parse::<Uuid>().map(Value::from).map_err(|error| decode(error.to_string())),
        (Elem::Text, serde_json::Value::String(text)) => Ok(Value::from(text)),
        (Elem::Int, serde_json::Value::Number(number)) => number.as_i64().map(Value::from).ok_or_else(|| decode("not an integer".into())),
        (_, other) => Err(decode(format!("{other} is not a list item"))),
    };
    let items = items.map(|Json(items)| items.into_iter().map(item).collect::<Result<Vec<_>, _>>()).transpose()?;
    Ok(Value::Array(elem.array_type(), items.map(Box::new)))
}

#[cfg(feature = "sqlite")]
/// SQLite holds a decimal as text.
fn text_decimal(row: &sqlx::sqlite::SqliteRow, name: &str) -> Result<Option<rust_decimal::Decimal>, sqlx::Error> {
    let text = row.try_get::<Option<String>, _>(name)?;
    text.map(|text| text.parse().map_err(|error| sqlx::Error::Decode(Box::new(error)))).transpose()
}

#[cfg(feature = "sqlite")]
fn array_as_json(value: Value) -> Value {
    match value {
        Value::Array(_, items) => Value::Json(items.map(|items| Box::new(items.into_iter().map(json_of).collect()))),
        other => other,
    }
}
