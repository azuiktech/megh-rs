//! Resource: a room, chair, seat, table or pod that an appointment takes up.

use axum::extract::State;
use nutype::nutype;
use serde::{Deserialize, Serialize};
use sqlx::QueryBuilder;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::value::{Attributes, Name};
use crate::auth::filter::push_filter;
use crate::auth::rest::{fetch_page, ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::{Entity, Table, TableEntity};

/// How many appointments fit at the same time; double booking is a capacity of 2.
#[nutype(
    validate(greater_or_equal = 1),
    default = 1,
    derive(Debug, Clone, Copy, PartialEq, Eq, Default, TryFrom, Into, Serialize, Deserialize),
)]
pub struct Capacity(i32);

/// How many appointments may wait for a place.
#[nutype(
    validate(greater_or_equal = 0),
    default = 0,
    derive(Debug, Clone, Copy, PartialEq, Eq, Default, TryFrom, Into, Serialize, Deserialize),
)]
pub struct Waitlist(i32);

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

const BY_KEY: &str = "SELECT * FROM resources WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL";

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(list, create)).routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(get, path = "/orgs/{org_id}/resources", tag = "resources",
    params(("org_id" = Uuid, Path), ("filter" = Option<String>, Query, description = "AIP-160 filter"),
           ("page_size" = Option<u16>, Query), ("page_token" = Option<String>, Query)),
    responses((status = 200, body = Page<Entity<Uuid, ResourceData>>)))]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Resource>> {
    let mut query = QueryBuilder::new("SELECT * FROM resources WHERE deleted_at IS NULL AND org_id = ");
    query.push_bind(request.org_id);
    if let Some(filter) = &request.filter {
        push_filter(&mut query, ResourceData::COLUMNS, filter)?;
    }
    fetch_page(&ctx.pool, query, &request.page).await
}

#[utoipa::path(get, path = "/orgs/{org_id}/resources/{id}", tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)))]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Resource> {
    sqlx::query_as(BY_KEY).bind(key.org_id).bind(key.id).fetch_optional(&ctx.pool).await?.ok_or(ApiError::NotFound)
}

#[utoipa::path(post, path = "/orgs/{org_id}/resources", tag = "resources",
    params(("org_id" = Uuid, Path)), request_body = ResourceData,
    responses((status = 200, body = Entity<Uuid, ResourceData>)))]
async fn create(State(ctx): State<AppContext>, Input(data): Input<ResourceData>) -> ApiResult<Resource> {
    Ok(Entity::new(Uuid::now_v7(), data).insert(&ctx.pool).await?)
}

#[utoipa::path(put, path = "/orgs/{org_id}/resources/{id}", tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)), request_body = ResourceData,
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)))]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<ResourceData>>) -> ApiResult<Resource> {
    Ok(TableEntity::update(&Entity::new(body.id, body.data), &ctx.pool).await?)
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(patch, path = "/orgs/{org_id}/resources/{id}", tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)), request_body = Object,
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)))]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Resource> {
    let mut transaction = ctx.pool.begin().await?;
    let current: Resource = sqlx::query_as(&format!("{BY_KEY} FOR UPDATE"))
        .bind(body.key.org_id)
        .bind(body.key.id)
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or(ApiError::NotFound)?;
    let changed = current.patch(&body.changes.into()).map_err(|error| ApiError::BadRequest(error.to_string()))?;
    let saved = TableEntity::update(&changed, &mut *transaction).await?;
    transaction.commit().await?;
    Ok(saved)
}

#[utoipa::path(delete, path = "/orgs/{org_id}/resources/{id}", tag = "resources",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ResourceData>), (status = 404)))]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Resource> {
    let current: Resource = sqlx::query_as(BY_KEY).bind(key.org_id).bind(key.id).fetch_optional(&ctx.pool).await?.ok_or(ApiError::NotFound)?;
    current.soft_delete(&ctx.pool).await?.ok_or(ApiError::NotFound)
}
