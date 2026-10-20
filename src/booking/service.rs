//! Service: what is booked. Setup and cleanup take only the room; each step takes the client and the
//! provider groups it names (a hygienist, then a hygienist and a dentist, then a hygienist).

use axum::extract::State;
use nutype::nutype;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::value::{Attributes, Duration, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::{Entity, Table};

/// A stretch of the client's time and the provider groups it needs (none, one or more).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Step {
    pub duration: Duration,
    #[serde(default)]
    #[schema(value_type = Vec<String>, example = json!(["hygienist"]))]
    pub providers: Vec<Name>,
}

/// The client's time: at least one step.
#[nutype(
    validate(predicate = |steps: &Vec<Step>| !steps.is_empty()),
    derive(Debug, Clone, PartialEq, AsRef, Serialize, Deserialize),
)]
pub struct Steps(Vec<Step>);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "services", keys = ["org_id", "id"])]
pub struct ServiceData {
    pub org_id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Tooth extraction")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub setup: Duration,
    #[serde(default)]
    pub cleanup: Duration,
    #[sqlx(json)]
    #[schema(value_type = Vec<Step>)]
    pub steps: Steps,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Service = Entity<Uuid, ServiceData>;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/services",
    tag = "services",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter"),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = Page<Entity<Uuid, ServiceData>>)),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Service>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/services/{id}",
    tag = "services",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ServiceData>), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Service> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/services",
    tag = "services",
    params(("org_id" = Uuid, Path)),
    request_body = ServiceData,
    responses((status = 200, body = Entity<Uuid, ServiceData>)),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<ServiceData>,
) -> ApiResult<Service> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/services/{id}",
    tag = "services",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ServiceData,
    responses((status = 200, body = Entity<Uuid, ServiceData>), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Keyed<ServiceData>>,
) -> ApiResult<Service> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/services/{id}",
    tag = "services",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = Entity<Uuid, ServiceData>), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Service> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/services/{id}",
    tag = "services",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ServiceData>), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Service> {
    crud::delete(&ctx.pool, &key).await
}
