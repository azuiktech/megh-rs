//! `/api/v1/orgs/{org}/clients`: the people, pets and others who are served.

use axum::extract::State;
use axum_params::Params;
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use validator::ValidateArgs;

use super::common::scope;
use super::page::paginate;
use super::AppContext;
use crate::auth::api::{ApiError, ApiResult, ErrorBody, ListQuery, Paginated};
use crate::booking::{Client, ClientId, ClientRow};
use crate::TableEntity;

#[derive(Deserialize)]
pub(super) struct ListClients {
    org: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct CreateClient {
    org: Uuid,
    #[serde(flatten)]
    client: Client,
}

#[derive(Deserialize)]
pub(super) struct ClientRef {
    org: Uuid,
    id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct ReplaceClient {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    client: Client,
}

#[derive(Deserialize)]
pub(super) struct PatchClient {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    changes: serde_json::Value,
}

async fn found(pool: &PgPool, scope: &[Uuid], id: Uuid) -> ApiResult<ClientRow> {
    let row = sqlx::query_as("SELECT * FROM clients WHERE id = $1 AND org_id = ANY($2)").bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("client {id} not found")))
}

async fn live(pool: &PgPool, scope: &[Uuid], id: Uuid) -> ApiResult<ClientRow> {
    let row = found(pool, scope, id).await?;
    match row.deleted_at {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("client {id} is deleted"))),
    }
}

/// List clients.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/clients", tag = "clients", params(("org" = Uuid, Path), ListQuery),
    responses((status = OK, body = Paginated<ClientRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListClients>) -> ApiResult<Paginated<ClientRow>> {
    let scope = scope(&ctx.pool, input.org).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let rows = sqlx::query_as(&format!("SELECT * FROM clients WHERE org_id = ANY($1){shown} ORDER BY created_at, id")).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

/// Create a client.
#[utoipa::path(post, path = "/api/v1/orgs/{org}/clients", tag = "clients", params(("org" = Uuid, Path)), request_body = Client,
    responses((status = OK, body = ClientRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn create(State(ctx): State<AppContext>, Params(input, _): Params<CreateClient>) -> ApiResult<ClientRow> {
    input.client.validate_with_args(&scope(&ctx.pool, input.org).await?)?;
    Ok(ClientRow::new(ClientId(Uuid::new_v4()), input.client).insert(&ctx.pool).await?)
}

/// Get a client.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/clients/{id}", tag = "clients", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = ClientRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<ClientRef>) -> ApiResult<ClientRow> {
    found(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await
}

/// Replace the data of a client.
#[utoipa::path(put, path = "/api/v1/orgs/{org}/clients/{id}", tag = "clients", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Client,
    responses((status = OK, body = ClientRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn replace(State(ctx): State<AppContext>, Params(input, _): Params<ReplaceClient>) -> ApiResult<ClientRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let current = live(&ctx.pool, &scope, input.id).await?;
    input.client.validate_with_args(&scope)?;
    Ok(ClientRow::with_timestamps(current.id, input.client, current.created_at, Utc::now()).save(&ctx.pool).await?)
}

/// Change the fields of a client that the body names (JSON merge patch, RFC 7396).
#[utoipa::path(patch, path = "/api/v1/orgs/{org}/clients/{id}", tag = "clients", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Client,
    responses((status = OK, body = ClientRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn patch(State(ctx): State<AppContext>, Params(input, _): Params<PatchClient>) -> ApiResult<ClientRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let patched = live(&ctx.pool, &scope, input.id).await?.patch(&input.changes).map_err(|error| ApiError::invalid(error.to_string()))?;
    patched.validate_with_args(&scope)?;
    Ok(patched.save(&ctx.pool).await?)
}

/// Delete a client. Soft delete: it comes back with `deleted_at`.
#[utoipa::path(delete, path = "/api/v1/orgs/{org}/clients/{id}", tag = "clients", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = ClientRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn remove(State(ctx): State<AppContext>, Params(input, _): Params<ClientRef>) -> ApiResult<ClientRow> {
    let current = live(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await?;
    current.soft_delete(&ctx.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))
}
