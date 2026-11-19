//! Tax: what a tenant sets up once for its invoices. A percentage for now; a fixed amount (a voucher applied to a
//! payment) comes later.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::value::Percent;
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};
use crate::booking::value::{Attributes, Name};

/// A percentage tax on a line (Odoo `account.tax`, Zoho `Tax`, HubSpot `taxes`).
#[crate::entity(table = "billing_taxes", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema)]
#[schema(example = json!({"name": "Sales tax", "rate": "7.5"}))]
pub struct Tax {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    #[schema(value_type = String, example = "Sales tax")]
    pub name: Name,
    pub rate: Percent,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/taxes",
    tag = "taxes",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Tax>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Tax>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/taxes/{id}",
    tag = "taxes",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Tax)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Tax> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/taxes",
    tag = "taxes",
    params(("tenant_id" = Uuid, Path)),
    request_body = Tax,
    responses((status = 200, body = inline(Tax))),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<Tax>) -> ApiResult<Tax> {
    crud::create(&ctx.pool, data, None).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/taxes/{id}",
    tag = "taxes",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Tax,
    responses((status = 200, body = inline(Tax)), (status = 404)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Tax>) -> ApiResult<Tax> {
    crud::replace(&ctx.pool, body, None).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/taxes/{id}",
    tag = "taxes",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Tax)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Tax> {
    crud::patch(&ctx.pool, body, None).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/taxes/{id}",
    tag = "taxes",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Tax)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Tax> {
    crud::delete(&ctx.pool, &key, None).await
}
