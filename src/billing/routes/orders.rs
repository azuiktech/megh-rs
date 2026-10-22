//! Order: what a client asked to buy or the org asked a vendor for. Created as a draft; changed only while a draft.

use axum::extract::State;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::draft::{ensure_draft, refuse_changes, Document};
use crate::billing::order::{Order, OrderData, OrderStatus};
use crate::billing::party::ensure_party;
use sqlx::PgPool;
use crate::Entity;

/// What a new or replaced order must be: a draft, with a party of its kind in the org.
async fn check(pool: &PgPool, order: &OrderData) -> ApiResult<()> {
    (order.status == OrderStatus::Draft)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("an order is a draft until it is opened".into()))?;
    ensure_party(pool, order.org_id, order.kind.party(), order.party_id).await
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/orders",
    tag = "orders",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, OrderData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Order>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/orders/{id}",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, OrderData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Order> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/orders",
    tag = "orders",
    params(("org_id" = Uuid, Path)),
    request_body = OrderData,
    responses((status = 200, body = inline(Entity<Uuid, OrderData>)), (status = 400)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<OrderData>) -> ApiResult<Order> {
    check(&ctx.pool, &data).await?;
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/orders/{id}",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = OrderData,
    responses((status = 200, body = inline(Entity<Uuid, OrderData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<OrderData>>) -> ApiResult<Order> {
    ensure_draft(&ctx.pool, Document::Order, body.data.org_id, body.id).await?;
    check(&ctx.pool, &body.data).await?;
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/orders/{id}",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, OrderData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Order> {
    ensure_draft(&ctx.pool, Document::Order, body.key.org_id, body.key.id).await?;
    refuse_changes(&body, &["kind", "party_id", "status"])?;
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/orders/{id}",
    tag = "orders",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, OrderData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Order> {
    ensure_draft(&ctx.pool, Document::Order, key.org_id, key.id).await?;
    crud::delete(&ctx.pool, &key).await
}
