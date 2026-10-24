//! Invoice: a request for payment from a customer, which is an org. A draft is edited freely; `:open` gives it its
//! number and fixes its totals, and an open invoice is not changed. Whether it is paid or overdue follows from
//! `amount_paid`, `balance_due` and `due_date`.

use axum::extract::State;
use axum::routing::post;
use axum::Extension;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::draft::{ensure_draft, refuse_changes};
use super::ids::Formats;
use super::lifecycle;
use super::value::{Currency, Micros};
use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::booking::value::{Address, Attributes, Name};
use crate::{Entity, Table};

/// HubSpot's `hs_invoice_status` without `paid`, which follows from the amounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Draft,
    Open,
    Void,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_invoices", keys = ["org_id", "id"])]
pub struct InvoiceData {
    /// The seller.
    pub org_id: Uuid,
    /// The customer: an org.
    pub customer_id: Uuid,
    /// Made by the app's `Ids` when the invoice is opened.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub status: InvoiceStatus,
    pub date: NaiveDate,
    pub due_date: NaiveDate,
    pub currency: Currency,
    /// The customer's address, copied when the invoice is opened.
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub bill_to: Option<Address>,
    #[serde(default)]
    pub comments: String,
    /// Worked out from the lines and fixed when the invoice is opened (HubSpot stores the same amounts).
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub tax_total: Micros,
    #[serde(default)]
    pub total: Micros,
    #[serde(default)]
    pub amount_paid: Micros,
    #[serde(default)]
    pub balance_due: Micros,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Invoice = Entity<Uuid, InvoiceData>;

/// What a new or replaced invoice must be: a draft, and without a number, which `:open` gives it.
fn check(invoice: &InvoiceData) -> ApiResult<()> {
    (invoice.status == InvoiceStatus::Draft)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("an invoice is a draft until it is opened".into()))?;
    invoice.number.is_none().then_some(()).ok_or_else(|| ApiError::BadRequest("the number is made when the invoice is opened".into()))
}

/// The custom methods; the path segment is `:open` or `:void`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub enum Verb {
    #[serde(rename = ":open")]
    Open,
    #[serde(rename = ":void")]
    Void,
}

#[derive(Debug, Deserialize, Validate)]
pub struct Action {
    #[serde(flatten)]
    pub key: Key,
    pub verb: Verb,
}

#[derive(OpenApi)]
#[openapi(paths(act))]
struct Actions;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Actions::openapi())
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
        .route("/orgs/{org_id}/invoices/{id}/{verb}", post(act))
}

/// `:open` a draft: it needs a line, takes its number from the app's `Ids`, works out the totals of its lines and
/// of itself, keeps its `bill_to`, owes its total and is no longer changed. `:void` an open invoice nothing was paid
/// on; it keeps its number.
#[utoipa::path(
    post,
    path = "/orgs/{org_id}/invoices/{id}:{verb}",
    tag = "invoices",
    params(
        ("org_id" = Uuid, Path),
        ("id" = Uuid, Path),
        ("verb" = String, Path, description = "open or void"),
    ),
    responses((status = 200, body = inline(Entity<Uuid, InvoiceData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn act(State(ctx): State<AppContext>, Extension(formats): Extension<Formats>, Input(action): Input<Action>) -> ApiResult<Invoice> {
    match action.verb {
        Verb::Open => lifecycle::open(&ctx.pool, &formats, &action.key).await,
        Verb::Void => lifecycle::void(&ctx.pool, &action.key).await,
    }
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/invoices",
    tag = "invoices",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, InvoiceData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Invoice>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/invoices/{id}",
    tag = "invoices",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, InvoiceData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Invoice> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/invoices",
    tag = "invoices",
    params(("org_id" = Uuid, Path)),
    request_body = InvoiceData,
    responses((status = 200, body = inline(Entity<Uuid, InvoiceData>)), (status = 400), (status = 409)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<InvoiceData>) -> ApiResult<Invoice> {
    check(&data)?;
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/invoices/{id}",
    tag = "invoices",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = InvoiceData,
    responses((status = 200, body = inline(Entity<Uuid, InvoiceData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<InvoiceData>>) -> ApiResult<Invoice> {
    ensure_draft::<InvoiceData>(&ctx.pool, body.data.org_id, body.id).await?;
    check(&body.data)?;
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/invoices/{id}",
    tag = "invoices",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, InvoiceData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Invoice> {
    ensure_draft::<InvoiceData>(&ctx.pool, body.key.org_id, body.key.id).await?;
    refuse_changes(&body, &["status", "customer_id", "number"])?;
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/invoices/{id}",
    tag = "invoices",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, InvoiceData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Invoice> {
    ensure_draft::<InvoiceData>(&ctx.pool, key.org_id, key.id).await?;
    crud::delete(&ctx.pool, &key).await
}
