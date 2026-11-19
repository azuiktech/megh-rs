//! Create, read, list, replace, patch and delete rows of any `#[entity]`, on any database that is a `Backend`, with a
//! pool, a transaction or a connection.
//!
//! The system sets `created_at`, `updated_at` and `deleted_at`, and `create` generates the last key (the id): what an
//! entity holds in them is never written. Every write is one statement (`UPDATE … RETURNING`); a row is read first only to
//! audit it, when the entity is audited.

use paginator_utils::{Cursor, CursorDirection, CursorValue};
use sea_query::{Alias, Condition, Expr, OnConflict, Query, SelectStatement, SimpleExpr, Value};
use sea_query_binder::SqlxBinder;
use serde::Serialize;
use sqlx::{Acquire, Transaction};
use uuid::Uuid;
use validator::Validate;

use crate::auth::audit::{self, AuditPolicy};
use crate::auth::rest::{ApiError, ApiResult, Changes, Key, ListRequest, Page};
use crate::auth::column::{Column, Kind};
use crate::auth::db::Backend;
use crate::auth::entity::Table;
use super::filter;

/// An entity the CRUD can run on from a handler: what it needs, in one name.
pub trait Row: Table + Validate + Serialize + Send + Sync + 'static {}

impl<T: Table + Validate + Serialize + Send + Sync + 'static> Row for T {}

const SYSTEM: [&str; 3] = ["created_at", "updated_at", "deleted_at"];

pub async fn get<'c, T, DB>(db: impl Acquire<'c, Database = DB>, key: &Key) -> ApiResult<T>
where
    T: Table,
    DB: Backend,
{
    let mut connection = db.begin().await.map_err(failed)?;
    one::<T, DB>(&mut connection, &base::<T>().cond_where(scope::<T>(key_values::<T>(key))).to_owned()).await
}

pub async fn list<'c, T, DB>(db: impl Acquire<'c, Database = DB>, request: &ListRequest) -> ApiResult<Page<T>>
where
    T: Table,
    DB: Backend,
{
    let order = *T::PRIMARY_KEY.last().expect("a table has a key");
    let size = request.page.size();
    let filter = request.filter.as_ref().map(filter::condition::<T, DB>).transpose()?;
    let last = request.page.page_token.as_deref().map(after).transpose()?;
    let query = base::<T>()
        .and_where(Expr::col(Alias::new(T::PRIMARY_KEY[0])).eq(Value::from(request.tenant_id)))
        .cond_where(Condition::all().add_option(filter))
        .and_where_option(last.map(|id| Expr::col(Alias::new(order)).gt(Value::from(id))))
        .order_by(Alias::new(order), sea_query::Order::Asc)
        .limit(u64::from(size) + 1)
        .to_owned();

    let mut connection = db.begin().await.map_err(failed)?;
    let mut items = many::<T, DB>(&mut connection, &query).await?;
    let has_more = items.len() > usize::from(size);
    items.truncate(usize::from(size));
    let next_page_token = items.last().filter(|_| has_more).map(|last| cursor(order, last)).transpose()?;
    Ok(Page { items, next_page_token })
}

/// The rows of the entity's table that are not deleted, narrowed by what `build` adds to the query.
pub async fn select<'c, T, DB>(db: impl Acquire<'c, Database = DB>, build: impl FnOnce(&mut SelectStatement)) -> ApiResult<Vec<T>>
where
    T: Table,
    DB: Backend,
{
    let mut query = base::<T>();
    build(&mut query);
    let mut connection = db.begin().await.map_err(failed)?;
    many::<T, DB>(&mut connection, &query).await
}

pub async fn create<'c, T, DB>(db: impl Acquire<'c, Database = DB>, data: T, actor: Option<&str>) -> ApiResult<T>
where
    T: Table + Validate + Serialize + Send,
    DB: Backend,
{
    data.validate().map_err(bad)?;
    let mut tx = db.begin().await.map_err(failed)?;
    let now = DB::now();
    let id: SimpleExpr = Expr::val(Uuid::now_v7()).into();
    let not_deleted: SimpleExpr = Expr::val(Kind::Timestamp.null()).into();
    let stamps = [("created_at", now.clone()), ("updated_at", now), ("deleted_at", not_deleted)];
    let stamps: Vec<(&str, SimpleExpr)> = stamps.into_iter().chain(T::PRIMARY_KEY.last().map(|last| (*last, id))).collect();
    let values: Vec<(&'static str, SimpleExpr)> = data
        .values()
        .into_iter()
        .map(|(name, value)| (name, stamps.iter().find(|(stamp, _)| *stamp == name).map_or_else(|| expr::<T>(name, value), |(_, set)| set.clone())))
        .collect();
    let insert = Query::insert()
        .into_table(Alias::new(T::TABLE_NAME))
        .columns(values.iter().map(|(name, _)| Alias::new(*name)))
        .values_panic(values.into_iter().map(|(_, value)| value))
        .returning(returning::<T>())
        .to_owned();
    let created = one::<T, DB>(&mut tx, &insert).await?;
    audit::record::<T, DB>(&mut tx, "INSERT", actor, None, Some(&created)).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(created)
}

/// Inserts the row as it is, or, when the key exists and the row is not deleted, sets every other column to the entity's
/// (`updated_at` to now): for a row whose id the caller decides.
pub async fn upsert<'c, T, DB>(db: impl Acquire<'c, Database = DB>, data: T) -> ApiResult<T>
where
    T: Table + Send,
    DB: Backend,
{
    let mut tx = db.begin().await.map_err(failed)?;
    let now = DB::now();
    let values: Vec<(&'static str, SimpleExpr)> = data
        .values()
        .into_iter()
        .map(|(name, value)| (name, match name { "updated_at" => now.clone(), _ => expr::<T>(name, value) }))
        .collect();
    let updated = values.iter().map(|(name, _)| *name).filter(|name| !T::PRIMARY_KEY.contains(name) && *name != "created_at");
    let alive = T::SOFT_DELETE.then(|| Expr::col((Alias::new(T::TABLE_NAME), Alias::new("deleted_at"))).is_null());
    let mut conflict = OnConflict::columns(T::PRIMARY_KEY.iter().map(|key| Alias::new(*key)));
    conflict.update_columns(updated.map(Alias::new));
    alive.into_iter().for_each(|alive| {
        conflict.action_and_where(alive);
    });
    let conflict = conflict.to_owned();
    let insert = Query::insert()
        .into_table(Alias::new(T::TABLE_NAME))
        .columns(values.iter().map(|(name, _)| Alias::new(*name)))
        .values_panic(values.into_iter().map(|(_, value)| value))
        .on_conflict(conflict)
        .returning(returning::<T>())
        .to_owned();
    let saved = one::<T, DB>(&mut tx, &insert).await?;
    tx.commit().await.map_err(failed)?;
    Ok(saved)
}

pub async fn replace<'c, T, DB>(db: impl Acquire<'c, Database = DB>, data: T, actor: Option<&str>) -> ApiResult<T>
where
    T: Table + Validate + Serialize + Send,
    DB: Backend,
{
    data.validate().map_err(bad)?;
    let mut tx = db.begin().await.map_err(failed)?;
    let values = data.values();
    let key: Vec<Value> = T::PRIMARY_KEY.iter().filter_map(|name| values.iter().find(|(column, _)| column == name)).map(|(_, value)| value.clone()).collect();
    let old = before::<T, DB>(&mut tx, &key).await?;
    let own = values.into_iter().filter(|(name, _)| !T::PRIMARY_KEY.contains(name) && !SYSTEM.contains(name));
    let sets = own.map(|(name, value)| (name, expr::<T>(name, value))).chain(stamps::<T>(&["updated_at"], &DB::now())).collect();
    let replaced = one::<T, DB>(&mut tx, &update::<T>(sets, key)).await?;
    audit::record::<T, DB>(&mut tx, "UPDATE", actor, old.as_ref(), Some(&replaced)).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(replaced)
}

/// `UPDATE … SET` exactly the columns the patch names, each replaced whole, and nothing else but `updated_at`.
pub async fn patch<'c, T, DB>(db: impl Acquire<'c, Database = DB>, body: Changes, actor: Option<&str>) -> ApiResult<T>
where
    T: Table + Validate + Serialize + Send,
    DB: Backend,
{
    let key = key_values::<T>(&body.key);
    let sets = body
        .changes
        .iter()
        .filter(|(name, _)| !SYSTEM.contains(&name.as_str()))
        .map(|(name, json)| {
            let column = T::COLUMNS.iter().find(|column| column.name == name).ok_or_else(|| ApiError::BadRequest(format!("unknown field {name}")))?;
            let value = match json {
                serde_json::Value::Null => DB::reset(),
                _ => expr::<T>(column.name, T::from_json(name, json).map_err(bad)?),
            };
            Ok((column.name, value))
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let mut tx = db.begin().await.map_err(failed)?;
    let old = before::<T, DB>(&mut tx, &key).await?;
    let sets = sets.into_iter().chain(stamps::<T>(&["updated_at"], &DB::now())).collect();
    let patched = one::<T, DB>(&mut tx, &update::<T>(sets, key)).await?;
    patched.validate().map_err(bad)?;
    audit::record::<T, DB>(&mut tx, "UPDATE", actor, old.as_ref(), Some(&patched)).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(patched)
}

/// Marks the row deleted, or removes it when the entity has no soft delete; answers with the row as it was left.
pub async fn delete<'c, T, DB>(db: impl Acquire<'c, Database = DB>, key: &Key, actor: Option<&str>) -> ApiResult<T>
where
    T: Table + Serialize + Send,
    DB: Backend,
{
    let key = key_values::<T>(key);
    let mut tx = db.begin().await.map_err(failed)?;
    let old = before::<T, DB>(&mut tx, &key).await?;
    let deleted = match T::SOFT_DELETE {
        true => {
            let sets = stamps::<T>(&["deleted_at", "updated_at"], &DB::now());
            let deleted = one::<T, DB>(&mut tx, &update::<T>(sets, key)).await?;
            DB::cascade(&mut tx, T::TABLE_NAME).await.map_err(failed)?;
            deleted
        }
        false => {
            let remove = Query::delete().from_table(Alias::new(T::TABLE_NAME)).cond_where(scope::<T>(key)).returning(returning::<T>()).to_owned();
            one::<T, DB>(&mut tx, &remove).await?
        }
    };
    audit::record::<T, DB>(&mut tx, "DELETE", actor, old.as_ref(), None::<&T>).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(deleted)
}

/// The row as it is, read only to be audited.
async fn before<T: Table, DB: Backend>(tx: &mut Transaction<'_, DB>, key: &[Value]) -> ApiResult<Option<T>> {
    match T::AUDIT {
        AuditPolicy::Disabled => Ok(None),
        _ => one::<T, DB>(tx, &base::<T>().cond_where(scope::<T>(key.to_vec())).to_owned()).await.map(Some),
    }
}

fn base<T: Table>() -> SelectStatement {
    Query::select().columns(T::COLUMNS.iter().map(|column| Alias::new(column.name))).from(Alias::new(T::TABLE_NAME)).cond_where(live::<T>()).to_owned()
}

fn returning<T: Table>() -> sea_query::ReturningClause {
    Query::returning().columns(T::COLUMNS.iter().map(|column| Alias::new(column.name)))
}

fn update<T: Table>(sets: Vec<(&'static str, SimpleExpr)>, key: Vec<Value>) -> sea_query::UpdateStatement {
    Query::update()
        .table(Alias::new(T::TABLE_NAME))
        .values(sets.into_iter().map(|(name, value)| (Alias::new(name), value)))
        .cond_where(scope::<T>(key))
        .returning(returning::<T>())
        .to_owned()
}

/// A value as the column takes it.
fn expr<T: Table>(name: &str, value: Value) -> SimpleExpr {
    match T::COLUMNS.iter().find(|column| column.name == name) {
        Some(column) => column.expr(value),
        None => Expr::val(value).into(),
    }
}

/// Rows that exist: not deleted, when the entity has a soft delete.
fn live<T: Table>() -> Condition {
    Condition::all().add_option(T::SOFT_DELETE.then(|| Expr::col(Alias::new("deleted_at")).is_null()))
}

fn scope<T: Table>(key: Vec<Value>) -> Condition {
    T::PRIMARY_KEY.iter().zip(key).fold(live::<T>(), |condition, (column, value)| condition.add(Expr::col(Alias::new(*column)).eq(value)))
}

/// The key's values for the table's key columns: (tenant, id), or the id alone for a table keyed by it.
fn key_values<T: Table>(key: &Key) -> Vec<Value> {
    let both: [Value; 2] = [key.tenant_id.into(), key.id.into()];
    both.into_iter().skip(2usize.saturating_sub(T::PRIMARY_KEY.len())).collect()
}

/// `names` of the entity's columns, each set to `value`.
fn stamps<T: Table>(names: &[&'static str], value: &SimpleExpr) -> Vec<(&'static str, SimpleExpr)> {
    names.iter().filter(|name| T::COLUMNS.iter().any(|column| column.name == **name)).map(|name| (*name, value.clone())).collect()
}

async fn many<T: Table, DB: Backend>(connection: &mut DB::Connection, statement: &impl SqlxBinder) -> ApiResult<Vec<T>> {
    let rows = DB::fetch(connection, DB::build(statement), T::COLUMNS).await.map_err(failed)?;
    rows.into_iter().map(|row| T::from_values(row).map_err(bad)).collect()
}

async fn one<T: Table, DB: Backend>(connection: &mut DB::Connection, statement: &impl SqlxBinder) -> ApiResult<T> {
    many::<T, DB>(connection, statement).await?.into_iter().next().ok_or(ApiError::NotFound)
}

fn after(token: &str) -> ApiResult<Uuid> {
    let invalid = || ApiError::BadRequest("invalid page_token".into());
    match Cursor::decode(token).map_err(|_| invalid())?.value {
        CursorValue::Uuid(id) => id.parse().map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

fn cursor<T: Table>(order: &str, last: &T) -> ApiResult<String> {
    let id = last.values().into_iter().find(|(name, _)| *name == order).and_then(|(_, value)| Uuid::from_value(value).ok());
    let id = id.ok_or_else(|| ApiError::BadRequest("the last key is not a uuid".into()))?;
    Cursor { field: order.to_string(), direction: CursorDirection::After, value: CursorValue::Uuid(id.to_string()) }.encode().map_err(ApiError::BadRequest)
}

fn bad(error: impl std::fmt::Display) -> ApiError {
    ApiError::BadRequest(error.to_string())
}

/// A constraint the database enforces on a value (a check, not null) is the caller's bad value.
fn failed(error: sqlx::Error) -> ApiError {
    match &error {
        sqlx::Error::Database(database) if matches!(database.kind(), sqlx::error::ErrorKind::CheckViolation | sqlx::error::ErrorKind::NotNullViolation) => {
            ApiError::BadRequest(database.to_string())
        }
        _ => error.into(),
    }
}

