//! `/api/v1/orgs/{org}/services`: what is offered, in steps, with variants.

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
use crate::booking::{Service, ServiceId, ServiceRow};
use crate::TableEntity;

#[derive(Deserialize)]
pub(super) struct ListServices {
    org: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct CreateService {
    org: Uuid,
    #[serde(flatten)]
    service: Service,
}

#[derive(Deserialize)]
pub(super) struct ServiceRef {
    org: Uuid,
    id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct ReplaceService {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    service: Service,
}

#[derive(Deserialize)]
pub(super) struct PatchService {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    changes: serde_json::Value,
}

async fn found(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<ServiceRow, ApiError> {
    let row = sqlx::query_as("SELECT * FROM services WHERE id = $1 AND org_id = ANY($2)").bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("service {id} not found")))
}

async fn live(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<ServiceRow, ApiError> {
    let row = found(pool, scope, id).await?;
    match row.deleted_at {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("service {id} is deleted"))),
    }
}

/// List XXX.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/services", tag = "services", params(("org" = Uuid, Path), ListQuery),
    responses((status = OK, body = Paginated<ServiceRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListServices>) -> ApiResult<Paginated<ServiceRow>> {
    let scope = scope(&ctx.pool, input.org).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let rows = sqlx::query_as(&format!("SELECT * FROM services WHERE org_id = ANY($1){shown} ORDER BY created_at, id")).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

/// Create a service.
#[utoipa::path(post, path = "/api/v1/orgs/{org}/services", tag = "services", params(("org" = Uuid, Path)), request_body = Service,
    responses((status = OK, body = ServiceRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn create(State(ctx): State<AppContext>, Params(input, _): Params<CreateService>) -> ApiResult<ServiceRow> {
    input.service.validate_with_args(&scope(&ctx.pool, input.org).await?)?;
    Ok(ServiceRow::new(ServiceId(Uuid::new_v4()), input.service).insert(&ctx.pool).await?)
}

/// Get a service.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/services/{id}", tag = "services", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = ServiceRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<ServiceRef>) -> ApiResult<ServiceRow> {
    found(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await
}

/// Replace the data of a service.
#[utoipa::path(put, path = "/api/v1/orgs/{org}/services/{id}", tag = "services", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Service,
    responses((status = OK, body = ServiceRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn replace(State(ctx): State<AppContext>, Params(input, _): Params<ReplaceService>) -> ApiResult<ServiceRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let current = live(&ctx.pool, &scope, input.id).await?;
    input.service.validate_with_args(&scope)?;
    Ok(ServiceRow::with_timestamps(current.id, input.service, current.created_at, Utc::now()).save(&ctx.pool).await?)
}

/// Change the fields of a service that the body names (JSON merge patch, RFC 7396).
#[utoipa::path(patch, path = "/api/v1/orgs/{org}/services/{id}", tag = "services", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Service,
    responses((status = OK, body = ServiceRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn patch(State(ctx): State<AppContext>, Params(input, _): Params<PatchService>) -> ApiResult<ServiceRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let patched = live(&ctx.pool, &scope, input.id).await?.patch(&input.changes).map_err(|error| ApiError::invalid(error.to_string()))?;
    patched.validate_with_args(&scope)?;
    Ok(patched.save(&ctx.pool).await?)
}

/// Delete a service. Soft delete: it comes back with `deleted_at`.
#[utoipa::path(delete, path = "/api/v1/orgs/{org}/services/{id}", tag = "services", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = ServiceRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn remove(State(ctx): State<AppContext>, Params(input, _): Params<ServiceRef>) -> ApiResult<ServiceRow> {
    let current = live(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await?;
    current.soft_delete(&ctx.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))
}
