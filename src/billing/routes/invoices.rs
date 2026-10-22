//! Invoice: created as a draft, changed only while a draft.

use axum::extract::State;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::invoice::{Invoice, InvoiceData};
use crate::billing::draft::{ensure_draft, refuse_changes};
use crate::billing::invoice::InvoiceStatus;
use crate::Entity;

/// What a new or replaced invoice must be: a draft, and without a number, which `:open` gives it.
fn check(invoice: &InvoiceData) -> ApiResult<()> {
    (invoice.status == InvoiceStatus::Draft)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("an invoice is a draft until it is opened".into()))?;
    invoice.number.is_none().then_some(()).ok_or_else(|| ApiError::BadRequest("the number is made when the invoice is opened".into()))
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
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
