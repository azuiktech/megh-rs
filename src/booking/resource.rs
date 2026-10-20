//! Resource: a room, chair, seat, table or pod that an appointment takes up.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::value::{Attributes, Capacity, Name, Waitlist};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::{Entity, Table};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "resources", keys = ["org_id", "id"])]
pub struct ResourceData {
    pub org_id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Room 1")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
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

pub type Resource = Entity<Uuid, ResourceData>;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/resources",
    tag = "resources",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter"),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = Page<Entity<Uuid, ResourceData>>)),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Resource>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/resources/{id}",
    tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Resource> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/resources",
    tag = "resources",
    params(("org_id" = Uuid, Path)),
    request_body = ResourceData,
    responses((status = 200, body = Entity<Uuid, ResourceData>)),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<ResourceData>,
) -> ApiResult<Resource> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/resources/{id}",
    tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ResourceData,
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Keyed<ResourceData>>,
) -> ApiResult<Resource> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/resources/{id}",
    tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Resource> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/resources/{id}",
    tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Resource> {
    crud::delete(&ctx.pool, &key).await
}
