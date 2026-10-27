//! Client: who books an appointment, a person or a pet.

use axum::extract::State;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::value::{Address, Attributes, Contact, Gender, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::{Entity, Table};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "clients", keys = ["tenant_id", "id"])]
#[schema(example = json!({"first_name": "Ada", "last_name": "Lovelace", "name": "Ada", "gender": "female", "birth_date": "1990-12-10", "email": "ada@example.com"}))]
pub struct ClientData {
    pub tenant_id: Uuid,
    #[sqlx(try_from = "String")]
    pub first_name: Name,
    #[sqlx(try_from = "String")]
    pub last_name: Name,
    #[sqlx(try_from = "String")]
    pub name: Name,
    /// What it is, such as patient or supplier.
    #[serde(default)]
    #[schema(value_type = Option<String>, example = "supplier")]
    pub role: Option<Name>,
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
    path = "/tenants/{tenant_id}/clients",
    tag = "clients",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, ClientData>>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Client>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/clients/{id}",
    tag = "clients",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ClientData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Client> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/clients",
    tag = "clients",
    params(("tenant_id" = Uuid, Path)),
    request_body = ClientData,
    responses((status = 200, body = inline(Entity<Uuid, ClientData>))),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<ClientData>,
) -> ApiResult<Client> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/clients/{id}",
    tag = "clients",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ClientData,
    responses((status = 200, body = inline(Entity<Uuid, ClientData>)), (status = 404)),
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
    path = "/tenants/{tenant_id}/clients/{id}",
    tag = "clients",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = ClientData, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Entity<Uuid, ClientData>)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Client> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/clients/{id}",
    tag = "clients",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ClientData>)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Client> {
    crud::delete(&ctx.pool, &key).await
}
