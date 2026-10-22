//! PaymentTerm: days after the invoice date that payment is due.

use axum::extract::State;

use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::tax::{PaymentTerm, PaymentTermData};
use crate::Entity;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/paymentTerms",
    tag = "paymentTerms",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, PaymentTermData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<PaymentTerm>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/paymentTerms/{id}",
    tag = "paymentTerms",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, PaymentTermData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<PaymentTerm> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/paymentTerms",
    tag = "paymentTerms",
    params(("org_id" = Uuid, Path)),
    request_body = PaymentTermData,
    responses((status = 200, body = inline(Entity<Uuid, PaymentTermData>))),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<PaymentTermData>) -> ApiResult<PaymentTerm> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/paymentTerms/{id}",
    tag = "paymentTerms",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = PaymentTermData,
    responses((status = 200, body = inline(Entity<Uuid, PaymentTermData>)), (status = 404)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<PaymentTermData>>) -> ApiResult<PaymentTerm> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/paymentTerms/{id}",
    tag = "paymentTerms",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, PaymentTermData>)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<PaymentTerm> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/paymentTerms/{id}",
    tag = "paymentTerms",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, PaymentTermData>)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<PaymentTerm> {
    crud::delete(&ctx.pool, &key).await
}
