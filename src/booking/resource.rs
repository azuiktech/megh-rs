//! Resource: a room, chair, seat, table or pod that an appointment takes up.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::value::{Attributes, Capacity, Name, Waitlist};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};

#[crate::entity(table = "resources", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema)]
#[schema(example = json!({"name": "Room 1", "role": "operatory", "capacity": 1, "waitlist": 0, "attributes": {"sea_facing": true}}))]
pub struct Resource {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Room 1")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
    /// What it is, such as operatory; a service step names the role of the provider it needs.
    #[schema(value_type = Option<String>, example = "operatory")]
    pub role: Option<Name>,
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
    path = "/tenants/{tenant_id}/resources",
    tag = "resources",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Resource>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Resource>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/resources/{id}",
    tag = "resources",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Resource)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Resource> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/resources",
    tag = "resources",
    params(("tenant_id" = Uuid, Path)),
    request_body = Resource,
    responses((status = 200, body = inline(Resource))),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<Resource>,
) -> ApiResult<Resource> {
    crud::create(&ctx.pool, data, None).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/resources/{id}",
    tag = "resources",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Resource,
    responses((status = 200, body = inline(Resource)), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Resource>,
) -> ApiResult<Resource> {
    crud::replace(&ctx.pool, body, None).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/resources/{id}",
    tag = "resources",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = Resource, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Resource)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Resource> {
    crud::patch(&ctx.pool, body, None).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/resources/{id}",
    tag = "resources",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Resource)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Resource> {
    crud::delete(&ctx.pool, &key, None).await
}
