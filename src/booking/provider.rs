//! Provider: a person an appointment is with, such as a dentist, hygienist, stylist or teacher.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::value::{Attributes, Capacity, Contact, Name, Waitlist};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};

#[crate::entity(table = "providers", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema)]
#[schema(example = json!({"first_name": "Albert", "last_name": "Hall", "name": "Dr Albert", "role": "dentist", "email": "albert@example.com", "mobile_phone": "+15165550100"}))]
pub struct Provider {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Ada")]
    pub first_name: Name,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Lovelace")]
    pub last_name: Name,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Dr Lovelace")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
    /// What it is, such as hygienist; a service step names the role of the provider it needs.
    #[schema(value_type = Option<String>, example = "hygienist")]
    pub role: Option<Name>,
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub contact: Contact,
    #[serde(default)]
    #[sqlx(try_from = "i32")]
    #[schema(value_type = i32, minimum = 1, default = 1)]
    pub capacity: Capacity,
    #[serde(default)]
    #[sqlx(try_from = "i32")]
    #[schema(value_type = i32, minimum = 0, default = 0)]
    pub waitlist: Waitlist,
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
    path = "/tenants/{tenant_id}/providers",
    tag = "providers",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Provider>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Provider>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/providers/{id}",
    tag = "providers",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Provider)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Provider> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/providers",
    tag = "providers",
    params(("tenant_id" = Uuid, Path)),
    request_body = Provider,
    responses((status = 200, body = inline(Provider))),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<Provider>,
) -> ApiResult<Provider> {
    crud::create(&ctx.pool, data, None).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/providers/{id}",
    tag = "providers",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Provider,
    responses((status = 200, body = inline(Provider)), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Provider>,
) -> ApiResult<Provider> {
    crud::replace(&ctx.pool, body, None).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/providers/{id}",
    tag = "providers",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = Provider, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Provider)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Provider> {
    crud::patch(&ctx.pool, body, None).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/providers/{id}",
    tag = "providers",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Provider)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Provider> {
    crud::delete(&ctx.pool, &key, None).await
}
