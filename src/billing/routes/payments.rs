//! Payment: money received for an open invoice.

use axum::extract::State;
use axum::Extension;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::payment::{Payment, PaymentData};
use crate::billing::balance;
use crate::billing::ids::Formats;
use crate::Entity;

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
