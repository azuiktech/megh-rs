//! Create, read, list, replace, patch and delete rows of any `#[entity]`, on a pool or in a transaction, on every driver.
//!
//! The system sets `created_at`, `updated_at` and `deleted_at`, and `create` generates the last key (the id): what an
//! entity holds in them is never written. Every write is one statement (`… RETURNING`); a row is read first only to
//! audit it, when the entity is audited. Fields are bound and rows read through each field's own `sqlx::Type`.

use chrono::{DateTime, Utc};
use paginator_utils::{Cursor, CursorDirection, CursorValue};
use serde::Serialize;
use uuid::Uuid;
use validator::Validate;

use crate::auth::audit::{self, AuditPolicy};
use crate::auth::db::{Db, On, QueryAs, Tx, Value};
use crate::auth::entity::{Table, Where};
use crate::auth::rest::{ApiError, ApiResult, Changes, Key, ListRequest, Page};
use super::filter;

/// An entity the CRUD can run on from a handler: what it needs, in one name.
pub trait Row: Table + Validate + Serialize + Send + Sync {}

impl<T: Table + Validate + Serialize + Send + Sync> Row for T {}

const SYSTEM: [&str; 3] = ["created_at", "updated_at", "deleted_at"];

pub async fn get<T: Table>(db: impl Into<On<'_>>, key: &Key) -> ApiResult<T> {
    select_where::<T>(db, scope::<T>(key)).await?.into_iter().next().ok_or(ApiError::NotFound)
}

pub async fn list<T: Table + Serialize>(db: impl Into<On<'_>>, request: &ListRequest) -> ApiResult<Page<T>> {
    let order = *T::PRIMARY_KEY.last().expect("a table has a key");
    let size = request.page.size();
    let filter = request.filter.as_ref().map(filter::condition::<T>).transpose()?;
    let last = request.page.page_token.as_deref().map(after).transpose()?;
    let tenant = Where::new(format!("\"{}\" = ?", T::PRIMARY_KEY[0]), vec![Box::new(request.tenant_id)]);
    let after = last.map(|id| Where::new(format!("\"{order}\" > ?"), vec![Box::new(id)]));
    let condition = [filter, after].into_iter().flatten().fold(tenant.and(live::<T>()), Where::and);
    let tail = format!(" ORDER BY \"{order}\" LIMIT {}", u64::from(size) + 1);
    let mut items: Vec<T> = statement(|condition| format!("{} WHERE {condition}{tail}", base::<T>()), condition).fetch_all(db).await.map_err(failed)?;
    let has_more = items.len() > usize::from(size);
    items.truncate(usize::from(size));
    let next_page_token = items.last().filter(|_| has_more).map(|last| cursor(order, last)).transpose()?;
    Ok(Page { items, next_page_token })
}

/// The rows of the entity's table that are not deleted and match `filter`.
pub async fn select<T: Table>(db: impl Into<On<'_>>, filter: Where) -> ApiResult<Vec<T>> {
    select_where(db, live::<T>().and(filter)).await
}

pub async fn create<T: Row>(db: impl Into<On<'_>>, data: T, actor: Option<&str>) -> ApiResult<T> {
    data.validate().map_err(bad)?;
    let now = Utc::now();
    let id = T::PRIMARY_KEY.last().copied();
    let params = T::COLUMNS.iter().zip(data.params()).map(|(column, param)| match *column {
        "created_at" | "updated_at" => Box::new(now) as Value,
        "deleted_at" => Box::new(None::<DateTime<Utc>>),
        name if Some(name) == id => Box::new(Uuid::now_v7()),
        _ => param,
    });
    let insert = format!("INSERT INTO \"{}\" ({}) VALUES ({}) RETURNING {}", T::TABLE_NAME, columns(T::COLUMNS), marks(T::COLUMNS.len()), columns(T::COLUMNS));
    let mut tx = db.into().transaction().await.map_err(failed)?;
    let created: T = Db::query_as(insert).bind_all(params.collect()).fetch_one(&mut *tx).await.map_err(failed)?;
    audit::record(&mut tx, "INSERT", actor, None, Some(&created)).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(created)
}

/// Inserts the row as it is, or, when the key exists and the row is not deleted, sets every other column to the entity's
/// (`updated_at` to now): for a row whose id the caller decides.
pub async fn upsert<T: Table>(db: impl Into<On<'_>>, data: T) -> ApiResult<T> {
    let now = Utc::now();
    let params = T::COLUMNS.iter().zip(data.params()).map(|(column, param)| match *column {
        "updated_at" => Box::new(now) as Value,
        _ => param,
    });
    let updated = T::COLUMNS.iter().filter(|name| !T::PRIMARY_KEY.contains(name) && **name != "created_at");
    let sets = updated.map(|name| format!("\"{name}\" = excluded.\"{name}\"")).collect::<Vec<_>>().join(", ");
    let alive = T::SOFT_DELETE.then(|| format!(" WHERE \"{}\".deleted_at IS NULL", T::TABLE_NAME)).unwrap_or_default();
    let insert = format!(
        "INSERT INTO \"{}\" ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {sets}{alive} RETURNING {}",
        T::TABLE_NAME,
        columns(T::COLUMNS),
        marks(T::COLUMNS.len()),
        columns(T::PRIMARY_KEY),
        columns(T::COLUMNS),
    );
    Db::query_as(insert).bind_all(params.collect()).fetch_one(db).await.map_err(failed)
}

pub async fn replace<T: Row>(db: impl Into<On<'_>>, data: T, actor: Option<&str>) -> ApiResult<T> {
    data.validate().map_err(bad)?;
    let key = key_of(&data)?;
    let own = T::COLUMNS.iter().zip(data.params()).filter(|(name, _)| !T::PRIMARY_KEY.contains(name) && !SYSTEM.contains(name));
    let (names, params): (Vec<_>, Vec<_>) = own.map(|(name, param)| (format!("\"{name}\" = ?"), param)).unzip();
    let mut tx = db.into().transaction().await.map_err(failed)?;
    let old = before::<T>(&mut tx, &key).await?;
    let replaced = update::<T>(&mut tx, Sets::new(names.join(", "), params), &key, Utc::now()).await?;
    audit::record(&mut tx, "UPDATE", actor, old.as_ref(), Some(&replaced)).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(replaced)
}

/// `UPDATE … SET` exactly the columns the patch names, each replaced whole, and nothing else but `updated_at`. `null`
/// sets the column's default where the driver can say so (PostgreSQL), else `NULL`.
pub async fn patch<T: Row>(db: impl Into<On<'_>>, body: Changes, actor: Option<&str>) -> ApiResult<T> {
    let changes = body.changes.iter().filter(|(name, _)| !SYSTEM.contains(&name.as_str()));
    let sets = changes.map(|(name, json)| change::<T>(name, json)).collect::<ApiResult<Vec<_>>>()?;
    let sets = sets.into_iter().fold(Sets::default(), Sets::and);
    let mut tx = db.into().transaction().await.map_err(failed)?;
    let old = before::<T>(&mut tx, &body.key).await?;
    let patched = update::<T>(&mut tx, sets, &body.key, Utc::now()).await?;
    patched.validate().map_err(bad)?;
    audit::record(&mut tx, "UPDATE", actor, old.as_ref(), Some(&patched)).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(patched)
}

/// Marks the row deleted, or removes it when the entity has no soft delete; answers with the row as it was left.
pub async fn delete<T: Row>(db: impl Into<On<'_>>, key: &Key, actor: Option<&str>) -> ApiResult<T> {
    let mut tx = db.into().transaction().await.map_err(failed)?;
    let old = before::<T>(&mut tx, key).await?;
    let deleted = match T::SOFT_DELETE {
        true => soft_delete::<T>(&mut tx, key).await?,
        false => {
            let remove = format!("DELETE FROM \"{}\" WHERE {{condition}} RETURNING {}", T::TABLE_NAME, columns(T::COLUMNS));
            statement(|condition| remove.replace("{condition}", condition), scope::<T>(key)).fetch_optional(&mut tx).await.map_err(failed)?.ok_or(ApiError::NotFound)?
        }
    };
    audit::record(&mut tx, "DELETE", actor, old.as_ref(), None::<&T>).await.map_err(failed)?;
    tx.commit().await.map_err(failed)?;
    Ok(deleted)
}

async fn soft_delete<T: Table>(tx: &mut Tx, key: &Key) -> ApiResult<T> {
    let now = Utc::now();
    let sets = Sets::new("\"deleted_at\" = ?".into(), vec![Box::new(now)]);
    let deleted = update::<T>(tx, sets, key, now).await?;
    crate::soft_delete::cascade(tx, T::TABLE_NAME, now).await.map_err(failed)?;
    Ok(deleted)
}

/// Assignments of an `UPDATE`: the text of each driver and the values bound to them.
#[derive(Default)]
struct Sets {
    standard: Vec<String>,
    postgres: Vec<String>,
    params: Vec<Value>,
}

impl Sets {
    fn new(text: String, params: Vec<Value>) -> Self {
        Sets { standard: vec![text.clone()], postgres: vec![text], params }
    }

    fn and(mut self, other: Sets) -> Sets {
        self.standard.extend(other.standard);
        self.postgres.extend(other.postgres);
        self.params.extend(other.params);
        self
    }
}

/// The assignment a patch makes to `name`: its value as the field's type takes it, or its default for `null`.
fn change<T: Table>(name: &str, json: &serde_json::Value) -> ApiResult<Sets> {
    let column = T::COLUMNS.iter().find(|column| **column == name).ok_or_else(|| ApiError::BadRequest(format!("unknown field {name}")))?;
    match json {
        serde_json::Value::Null => Ok(Sets { standard: vec![format!("\"{column}\" = NULL")], postgres: vec![format!("\"{column}\" = DEFAULT")], params: vec![] }),
        _ => Ok(Sets::new(format!("\"{column}\" = ?"), vec![T::param(column, json).map_err(bad)?])),
    }
}

/// `UPDATE` the row of `key` with `sets` and `updated_at` set to now.
async fn update<T: Table>(tx: &mut Tx, sets: Sets, key: &Key, now: DateTime<Utc>) -> ApiResult<T> {
    let stamp = T::COLUMNS.contains(&"updated_at").then(|| Sets::new("\"updated_at\" = ?".into(), vec![Box::new(now)]));
    let Sets { standard, postgres, params } = stamp.into_iter().fold(sets, Sets::and);
    let Where { standard: condition, postgres: pg_condition, sqlite: sqlite_condition, params: key_params } = scope::<T>(key);
    let text = |sets: &[String], condition: &str| format!("UPDATE \"{}\" SET {} WHERE {condition} RETURNING {}", T::TABLE_NAME, sets.join(", "), columns(T::COLUMNS));
    Db::query_as(text(&standard, &condition))
        .postgres(text(&postgres, &pg_condition))
        .sqlite(text(&standard, &sqlite_condition))
        .bind_all(params)
        .bind_all(key_params)
        .fetch_optional(&mut *tx)
        .await
        .map_err(failed)?
        .ok_or(ApiError::NotFound)
}

/// The row as it is, read only to be audited.
async fn before<T: Table>(tx: &mut Tx, key: &Key) -> ApiResult<Option<T>> {
    match T::AUDIT {
        AuditPolicy::Disabled => Ok(None),
        _ => select_where::<T>(&mut *tx, scope::<T>(key)).await.map(|rows| rows.into_iter().next()),
    }
}

async fn select_where<T: Table>(db: impl Into<On<'_>>, condition: Where) -> ApiResult<Vec<T>> {
    statement(|condition| format!("{} WHERE {condition}", base::<T>()), condition).fetch_all(db).await.map_err(failed)
}

/// A statement around `condition`, for each driver's text of it.
fn statement<T>(text: impl Fn(&str) -> String, condition: Where) -> QueryAs<'static, T> {
    let Where { standard, postgres, sqlite, params } = condition;
    Db::query_as(text(&standard)).postgres(text(&postgres)).sqlite(text(&sqlite)).bind_all(params)
}

fn base<T: Table>() -> String {
    format!("SELECT {} FROM \"{}\"", columns(T::COLUMNS), T::TABLE_NAME)
}

fn columns(names: &[&str]) -> String {
    names.iter().map(|name| format!("\"{name}\"")).collect::<Vec<_>>().join(", ")
}

fn marks(count: usize) -> String {
    vec!["?"; count].join(", ")
}

/// Rows that exist: not deleted, when the entity has a soft delete.
fn live<T: Table>() -> Where {
    T::SOFT_DELETE.then(|| Where::new("deleted_at IS NULL", vec![])).unwrap_or_else(Where::all)
}

/// The row of `key`, if it exists: (tenant, id), or the id alone for a table keyed by it.
fn scope<T: Table>(key: &Key) -> Where {
    let values: [Value; 2] = [Box::new(key.tenant_id), Box::new(key.id)];
    let skip = 2usize.saturating_sub(T::PRIMARY_KEY.len());
    T::PRIMARY_KEY.iter().zip(values.into_iter().skip(skip)).fold(live::<T>(), |condition, (column, value)| condition.and(Where::new(format!("\"{column}\" = ?"), vec![value])))
}

/// The key of an entity: its tenant and the last key column, both uuids.
fn key_of<T: Table + Serialize>(data: &T) -> ApiResult<Key> {
    let json = serde_json::to_value(data).map_err(bad)?;
    let uuid = |name: &str| json.get(name).and_then(|value| value.as_str()).and_then(|text| text.parse::<Uuid>().ok());
    let id = T::PRIMARY_KEY.last().and_then(|name| uuid(name)).ok_or_else(|| ApiError::BadRequest("the key is not a uuid".into()))?;
    let tenant_id = T::PRIMARY_KEY.first().filter(|_| T::PRIMARY_KEY.len() > 1).and_then(|name| uuid(name)).unwrap_or(id);
    Ok(Key { tenant_id, id })
}

fn after(token: &str) -> ApiResult<Uuid> {
    let invalid = || ApiError::BadRequest("invalid page_token".into());
    match Cursor::decode(token).map_err(|_| invalid())?.value {
        CursorValue::Uuid(id) => id.parse().map_err(|_| invalid()),
        _ => Err(invalid()),
    }
}

fn cursor<T: Serialize>(order: &str, last: &T) -> ApiResult<String> {
    let json = serde_json::to_value(last).map_err(bad)?;
    let id = json.get(order).and_then(|value| value.as_str()).and_then(|text| text.parse::<Uuid>().ok());
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
