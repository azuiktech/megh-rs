//! Invoice: created as a draft, changed only while a draft.

use axum::extract::State;
use axum::routing::post;
use axum::Extension;
use serde::Deserialize;
use utoipa::OpenApi;
use validator::Validate;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::invoice::{Invoice, InvoiceData};
use crate::billing::customer::ensure_customer;
use crate::billing::draft::{ensure_draft, refuse_changes};
use crate::billing::ids::Formats;
use crate::billing::lifecycle;
use crate::billing::invoice::InvoiceStatus;
use crate::Entity;
use sqlx::PgPool;

/// What a new or replaced invoice must be: a draft, without a number (which `:open` gives it), for an org that
/// belongs to the seller.
async fn check(pool: &PgPool, invoice: &InvoiceData) -> ApiResult<()> {
    (invoice.status == InvoiceStatus::Draft)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("an invoice is a draft until it is opened".into()))?;
    invoice.number.is_none().then_some(()).ok_or_else(|| ApiError::BadRequest("the number is made when the invoice is opened".into()))?;
    ensure_customer(pool, invoice.org_id, invoice.customer_id).await
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
    check(&ctx.pool, &data).await?;
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
    check(&ctx.pool, &body.data).await?;
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
