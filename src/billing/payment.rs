//! Payment: money received for one invoice (a receipt). A refund is kept on the payment, as HubSpot does.

use axum::extract::State;
use axum::Extension;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::balance;
use super::ids::Formats;
use super::value::{Micros, PaymentMethod};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::booking::value::{Attributes, Name};
use crate::{Entity, Table};

/// HubSpot's `hs_latest_status` without the refund and dispute states: the refund is the `refunded` amount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Processing,
    Succeeded,
    Failed,
    Void,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_payments", keys = ["org_id", "id"])]
#[validate(schema(function = "amounts"))]
pub struct PaymentData {
    pub org_id: Uuid,
    pub invoice_id: Uuid,
    /// Made by the app's `Ids` when the payment is recorded.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub date: NaiveDate,
    pub amount: Micros,
    pub method: PaymentMethod,
    pub status: PaymentStatus,
    /// How much of `amount` was given back.
    #[serde(default)]
    pub refunded: Micros,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Payment = Entity<Uuid, PaymentData>;

fn amounts(payment: &PaymentData) -> Result<(), ValidationError> {
    let (amount, refunded) = (i64::from(payment.amount), i64::from(payment.refunded));
    (amount > 0 && (0..=amount).contains(&refunded))
        .then_some(())
        .ok_or_else(|| ValidationError::new("the amount must be positive and the refunded part between nothing and all of it"))
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/payments",
    tag = "payments",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, PaymentData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Payment>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/payments/{id}",
    tag = "payments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Payment> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/payments",
    tag = "payments",
    params(("org_id" = Uuid, Path)),
    request_body = PaymentData,
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 400), (status = 409)),
)]
async fn create(State(ctx): State<AppContext>, Extension(formats): Extension<Formats>, Input(data): Input<PaymentData>) -> ApiResult<Payment> {
    balance::record(&ctx.pool, &formats, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/payments/{id}",
    tag = "payments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = PaymentData,
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<PaymentData>>) -> ApiResult<Payment> {
    balance::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/payments/{id}",
    tag = "payments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Payment> {
    balance::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/payments/{id}",
    tag = "payments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Payment> {
    balance::remove(&ctx.pool, &key).await
}
