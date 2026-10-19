//! `/api/v1/orgs/{org}/bookables`: providers and rooms.

use axum::extract::State;
use axum_params::Params;
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use validator::ValidateArgs;

use super::common::scope;
use crate::auth::api::{ApiError, ApiResult, ErrorBody, ListQuery, Paginated};
use super::page::paginate;
use super::AppContext;
use crate::booking::{Bookable, BookableId, BookableRow};
use crate::TableEntity;

#[derive(Deserialize)]
pub(super) struct ListBookables {
    org: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct CreateBookable {
    org: Uuid,
    #[serde(flatten)]
    bookable: Bookable,
}

#[derive(Deserialize)]
pub(super) struct BookableRef {
    org: Uuid,
    id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct ReplaceBookable {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    bookable: Bookable,
}

#[derive(Deserialize)]
pub(super) struct PatchBookable {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    changes: serde_json::Value,
}

async fn found(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<BookableRow, ApiError> {
    let row = sqlx::query_as("SELECT * FROM bookables WHERE id = $1 AND org_id = ANY($2)").bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("bookable {id} not found")))
}

async fn live(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<BookableRow, ApiError> {
    let row = found(pool, scope, id).await?;
    match row.deleted_at {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("bookable {id} is deleted"))),
    }
}

/// List bookables.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/bookables", tag = "bookables", params(("org" = Uuid, Path), ListQuery),
    responses((status = OK, body = Paginated<BookableRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListBookables>) -> ApiResult<Paginated<BookableRow>> {
    let scope = scope(&ctx.pool, input.org).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let rows = sqlx::query_as(&format!("SELECT * FROM bookables WHERE org_id = ANY($1){shown} ORDER BY created_at, id")).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

/// Create a bookable.
#[utoipa::path(post, path = "/api/v1/orgs/{org}/bookables", tag = "bookables", params(("org" = Uuid, Path)), request_body = Bookable,
    responses((status = OK, body = BookableRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn create(State(ctx): State<AppContext>, Params(input, _): Params<CreateBookable>) -> ApiResult<BookableRow> {
    input.bookable.validate_with_args(&scope(&ctx.pool, input.org).await?)?;
    Ok(BookableRow::new(BookableId(Uuid::new_v4()), input.bookable).insert(&ctx.pool).await?)
}

/// Get a bookable.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/bookables/{id}", tag = "bookables", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = BookableRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<BookableRef>) -> ApiResult<BookableRow> {
    found(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await
}

/// Replace the data of a bookable.
#[utoipa::path(put, path = "/api/v1/orgs/{org}/bookables/{id}", tag = "bookables", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Bookable,
    responses((status = OK, body = BookableRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn replace(State(ctx): State<AppContext>, Params(input, _): Params<ReplaceBookable>) -> ApiResult<BookableRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let current = live(&ctx.pool, &scope, input.id).await?;
    input.bookable.validate_with_args(&scope)?;
    Ok(BookableRow::with_timestamps(current.id, input.bookable, current.created_at, Utc::now()).save(&ctx.pool).await?)
}

/// Change the fields of a bookable that the body names (JSON merge patch, RFC 7396).
#[utoipa::path(patch, path = "/api/v1/orgs/{org}/bookables/{id}", tag = "bookables", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Bookable,
    responses((status = OK, body = BookableRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn patch(State(ctx): State<AppContext>, Params(input, _): Params<PatchBookable>) -> ApiResult<BookableRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let patched = live(&ctx.pool, &scope, input.id).await?.patch(&input.changes).map_err(|error| ApiError::invalid(error.to_string()))?;
    patched.validate_with_args(&scope)?;
    Ok(patched.save(&ctx.pool).await?)
}

/// Delete a bookable. Soft delete: it comes back with `deleted_at`.
#[utoipa::path(delete, path = "/api/v1/orgs/{org}/bookables/{id}", tag = "bookables", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = BookableRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn remove(State(ctx): State<AppContext>, Params(input, _): Params<BookableRef>) -> ApiResult<BookableRow> {
    let current = live(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await?;
    current.soft_delete(&ctx.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))
}
