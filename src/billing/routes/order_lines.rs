//! Order line: one product or service of an order. Changed only while its order is a draft.

use axum::extract::State;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::draft::{ensure_draft, ensure_line_draft, refuse_changes, Document};
use crate::billing::order::{OrderLine, OrderLineData};
use sqlx::PgPool;
use crate::Entity;

/// The line's order must be an order of the org (a bad request if not) and still a draft (a conflict if not).
async fn in_draft_order(pool: &PgPool, line: &OrderLineData) -> ApiResult<()> {
    match ensure_draft(pool, Document::Order, line.org_id, line.order_id).await {
        Err(ApiError::NotFound) => Err(ApiError::BadRequest("order_id is not an order of the org".into())),
        other => other,
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/orderLines",
    tag = "orderLines",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, OrderLineData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<OrderLine>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/orderLines/{id}",
    tag = "orderLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, OrderLineData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<OrderLine> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/orderLines",
    tag = "orderLines",
    params(("org_id" = Uuid, Path)),
    request_body = OrderLineData,
    responses((status = 200, body = inline(Entity<Uuid, OrderLineData>)), (status = 400)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<OrderLineData>) -> ApiResult<OrderLine> {
    in_draft_order(&ctx.pool, &data).await?;
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/orderLines/{id}",
    tag = "orderLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = OrderLineData,
    responses((status = 200, body = inline(Entity<Uuid, OrderLineData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<OrderLineData>>) -> ApiResult<OrderLine> {
    ensure_line_draft(&ctx.pool, Document::Order, body.data.org_id, body.id).await?;
    in_draft_order(&ctx.pool, &body.data).await?;
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/orderLines/{id}",
    tag = "orderLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, OrderLineData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<OrderLine> {
    ensure_line_draft(&ctx.pool, Document::Order, body.key.org_id, body.key.id).await?;
    refuse_changes(&body, &["order_id"])?;
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/orderLines/{id}",
    tag = "orderLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, OrderLineData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<OrderLine> {
    ensure_line_draft(&ctx.pool, Document::Order, key.org_id, key.id).await?;
    crud::delete(&ctx.pool, &key).await
}
