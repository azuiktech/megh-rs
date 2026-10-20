//! Create, read, list, replace, patch and delete rows of an org's table, keyed by `(org_id, id)`.
//!
//! Each route of an entity calls one of these; the route carries the entity's OpenAPI description.

use serde::Serialize;
use serde::de::DeserializeOwned;
use sqlx::postgres::PgRow;
use sqlx::{FromRow, PgPool, QueryBuilder};
use uuid::Uuid;
use validator::Validate;

use super::filter::push_filter;
use super::rest::{ApiError, ApiResult, Changes, Key, Keyed, ListRequest, Page, fetch_page};
use crate::{Entity, Table, TableEntity};

/// An entity's data stored in a table with `org_id`, `id`, timestamps and `deleted_at`.
pub trait Row:
    Table + Validate + for<'r> FromRow<'r, PgRow> + Serialize + DeserializeOwned + Send + Sync + Unpin + 'static
{
}

impl<T> Row for T where
    T: Table
        + Validate
        + for<'r> FromRow<'r, PgRow>
        + Serialize
        + DeserializeOwned
        + Send
        + Sync
        + Unpin
        + 'static
{
}

pub async fn list<T: Row>(
    pool: &PgPool,
    request: &ListRequest,
) -> ApiResult<Page<Entity<Uuid, T>>> {
    let mut query = QueryBuilder::new(format!(
        "SELECT * FROM {} WHERE deleted_at IS NULL AND org_id = ",
        T::TABLE_NAME
    ));
    query.push_bind(request.org_id);
    if let Some(filter) = &request.filter {
        push_filter(&mut query, T::COLUMNS, filter)?;
    }
    fetch_page(pool, query, &request.page).await
}

pub async fn get<T: Row>(pool: &PgPool, key: &Key) -> ApiResult<Entity<Uuid, T>> {
    sqlx::query_as(&by_key::<T>(""))
        .bind(key.org_id)
        .bind(key.id)
        .fetch_optional(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

pub async fn create<T: Row>(pool: &PgPool, data: T) -> ApiResult<Entity<Uuid, T>> {
    Ok(TableEntity::insert(&Entity::new(Uuid::now_v7(), data), pool).await?)
}

pub async fn replace<T: Row>(pool: &PgPool, body: Keyed<T>) -> ApiResult<Entity<Uuid, T>> {
    Ok(TableEntity::update(&Entity::new(body.id, body.data), pool).await?)
}

/// JSON merge patch (RFC 7396) on the stored row, read and written in one transaction.
pub async fn patch<T: Row>(pool: &PgPool, body: Changes) -> ApiResult<Entity<Uuid, T>> {
    let mut transaction = pool.begin().await?;
    let current: Entity<Uuid, T> = sqlx::query_as(&by_key::<T>(" FOR UPDATE"))
        .bind(body.key.org_id)
        .bind(body.key.id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ApiError::NotFound)?;
    let changed = current
        .patch(&body.changes.into())
        .map_err(|error| ApiError::BadRequest(error.to_string()))?;
    changed.data.validate().map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let saved = TableEntity::update(&changed, &mut *transaction).await?;
    transaction.commit().await?;
    Ok(saved)
}

/// Soft delete; answers with the deleted row.
pub async fn delete<T: Row>(pool: &PgPool, key: &Key) -> ApiResult<Entity<Uuid, T>> {
    get::<T>(pool, key)
        .await?
        .soft_delete(pool)
        .await?
        .ok_or(ApiError::NotFound)
}

fn by_key<T: Table>(suffix: &str) -> String {
    format!(
        "SELECT * FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL{suffix}",
        T::TABLE_NAME
    )
}
