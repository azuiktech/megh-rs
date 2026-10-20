//! Client: who books an appointment, a person or a pet.

use axum::extract::State;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use super::value::{Address, Attributes, Contact, Gender, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::{Entity, Table};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "clients", keys = ["org_id", "id"])]
pub struct ClientData {
    pub org_id: Uuid,
    #[sqlx(try_from = "String")]
    pub first_name: Name,
    #[sqlx(try_from = "String")]
    pub last_name: Name,
    #[sqlx(try_from = "String")]
    pub name: Name,
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub contact: Contact,
    #[serde(default = "unknown")]
    pub gender: Gender,
    #[serde(default)]
    pub birth_date: Option<NaiveDate>,
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub address: Option<Address>,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Client = Entity<Uuid, ClientData>;

fn unknown() -> Gender {
    Gender::Unknown
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/clients",
    tag = "clients",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter"),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = Page<Entity<Uuid, ClientData>>)),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Client>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/clients/{id}",
    tag = "clients",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ClientData>), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Client> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/clients",
    tag = "clients",
    params(("org_id" = Uuid, Path)),
    request_body = ClientData,
    responses((status = 200, body = Entity<Uuid, ClientData>)),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<ClientData>,
) -> ApiResult<Client> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/clients/{id}",
    tag = "clients",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ClientData,
    responses((status = 200, body = Entity<Uuid, ClientData>), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Keyed<ClientData>>,
) -> ApiResult<Client> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/clients/{id}",
    tag = "clients",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = Entity<Uuid, ClientData>), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Client> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/clients/{id}",
    tag = "clients",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ClientData>), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Client> {
    crud::delete(&ctx.pool, &key).await
}
