//! Payment: money received from a client or paid to a vendor.

use axum::extract::State;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::draft::refuse_changes;
use crate::billing::party::ensure_party;
use crate::billing::payment::{Payment, PaymentData};
use crate::Entity;
use sqlx::PgPool;
use uuid::Uuid;

/// What a new or replaced payment must be: a payment of a live client (inbound) or vendor (outbound) of the org.
async fn check(pool: &PgPool, payment: &PaymentData) -> ApiResult<()> {
    ensure_party(pool, payment.org_id, payment.kind.party(), payment.party_id).await
}

/// A payment that has applications is not deleted: they are removed first.
async fn ensure_unapplied(pool: &PgPool, org_id: Uuid, id: Uuid) -> ApiResult<()> {
    let applied: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM billing_applications WHERE org_id = $1 AND payment_id = $2 AND deleted_at IS NULL)")
        .bind(org_id)
        .bind(id)
        .fetch_one(pool)
        .await?;
    (!applied).then_some(()).ok_or_else(|| ApiError::Conflict("the payment has applications; remove them first".into()))
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
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 400)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<PaymentData>) -> ApiResult<Payment> {
    check(&ctx.pool, &data).await?;
    crud::create(&ctx.pool, data).await
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
    check(&ctx.pool, &body.data).await?;
    crud::replace(&ctx.pool, body).await
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
    refuse_changes(&body, &["kind", "party_id"])?;
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/payments/{id}",
    tag = "payments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, PaymentData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Payment> {
    ensure_unapplied(&ctx.pool, key.org_id, key.id).await?;
    crud::delete(&ctx.pool, &key).await
}
