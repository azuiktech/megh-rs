//! `/api/v1/orgs/{org}/paymentMethods`: passes, vouchers, cash and the like.

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
use crate::booking::{ClientId, PaymentMethod, PaymentMethodId, PaymentMethodRow};
use crate::TableEntity;

#[derive(Deserialize)]
pub(super) struct ListPaymentMethods {
    org: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct CreatePaymentMethod {
    org: Uuid,
    #[serde(flatten)]
    payment_method: PaymentMethod,
}

#[derive(Deserialize)]
pub(super) struct PaymentMethodRef {
    org: Uuid,
    id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct ReplacePaymentMethod {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    payment_method: PaymentMethod,
}

#[derive(Deserialize)]
pub(super) struct PatchPaymentMethod {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    changes: serde_json::Value,
}

/// The owner, when given, is a client of the covered orgs.
async fn owned(pool: &PgPool, scope: &[Uuid], owner: Option<ClientId>) -> Result<(), ApiError> {
    let Some(client) = owner else { return Ok(()) };
    let org: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM clients WHERE id = $1 AND deleted_at IS NULL").bind(client).fetch_optional(pool).await?;
    org.filter(|org| scope.contains(org)).map(drop).ok_or_else(|| ApiError::not_found(format!("clients {} not found", client.0)))
}

async fn found(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<PaymentMethodRow, ApiError> {
    let row = sqlx::query_as("SELECT * FROM payment_methods WHERE id = $1 AND org_id = ANY($2)").bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("payment method {id} not found")))
}

async fn live(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<PaymentMethodRow, ApiError> {
    let row = found(pool, scope, id).await?;
    match row.deleted_at {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("payment method {id} is deleted"))),
    }
}

/// List XXX.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/paymentMethods", tag = "paymentMethods", params(("org" = Uuid, Path), ListQuery),
    responses((status = OK, body = Paginated<PaymentMethodRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListPaymentMethods>) -> ApiResult<Paginated<PaymentMethodRow>> {
    let scope = scope(&ctx.pool, input.org).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let rows = sqlx::query_as(&format!("SELECT * FROM payment_methods WHERE org_id = ANY($1){shown} ORDER BY created_at, id")).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

/// Create a payment method.
#[utoipa::path(post, path = "/api/v1/orgs/{org}/paymentMethods", tag = "paymentMethods", params(("org" = Uuid, Path)), request_body = PaymentMethod,
    responses((status = OK, body = PaymentMethodRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn create(State(ctx): State<AppContext>, Params(input, _): Params<CreatePaymentMethod>) -> ApiResult<PaymentMethodRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    input.payment_method.validate_with_args(&scope)?;
    owned(&ctx.pool, &scope, input.payment_method.owner_id).await?;
    Ok(PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), input.payment_method).insert(&ctx.pool).await?)
}

/// Get a payment method.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/paymentMethods/{id}", tag = "paymentMethods", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = PaymentMethodRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<PaymentMethodRef>) -> ApiResult<PaymentMethodRow> {
    found(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await
}

/// Replace the data of a payment method.
#[utoipa::path(put, path = "/api/v1/orgs/{org}/paymentMethods/{id}", tag = "paymentMethods", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = PaymentMethod,
    responses((status = OK, body = PaymentMethodRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn replace(State(ctx): State<AppContext>, Params(input, _): Params<ReplacePaymentMethod>) -> ApiResult<PaymentMethodRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let current = live(&ctx.pool, &scope, input.id).await?;
    input.payment_method.validate_with_args(&scope)?;
    owned(&ctx.pool, &scope, input.payment_method.owner_id).await?;
    Ok(PaymentMethodRow::with_timestamps(current.id, input.payment_method, current.created_at, Utc::now()).save(&ctx.pool).await?)
}

/// Change the fields of a payment method that the body names (JSON merge patch, RFC 7396).
#[utoipa::path(patch, path = "/api/v1/orgs/{org}/paymentMethods/{id}", tag = "paymentMethods", params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = PaymentMethod,
    responses((status = OK, body = PaymentMethodRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn patch(State(ctx): State<AppContext>, Params(input, _): Params<PatchPaymentMethod>) -> ApiResult<PaymentMethodRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let patched = live(&ctx.pool, &scope, input.id).await?.patch(&input.changes).map_err(|error| ApiError::invalid(error.to_string()))?;
    patched.validate_with_args(&scope)?;
    owned(&ctx.pool, &scope, patched.owner_id).await?;
    Ok(patched.save(&ctx.pool).await?)
}

/// Delete a payment method. Soft delete: it comes back with `deleted_at`.
#[utoipa::path(delete, path = "/api/v1/orgs/{org}/paymentMethods/{id}", tag = "paymentMethods", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = PaymentMethodRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn remove(State(ctx): State<AppContext>, Params(input, _): Params<PaymentMethodRef>) -> ApiResult<PaymentMethodRow> {
    let current = live(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await?;
    current.soft_delete(&ctx.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))
}
