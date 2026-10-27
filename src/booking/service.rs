//! Service: what is booked. Setup and cleanup take only the room; each step takes the client and the
//! provider groups it names (a hygienist, then a hygienist and a dentist, then a hygienist).

use axum::extract::State;
use iso8601::Duration;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::price::{self, ItemKind, Priced};
use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::value::{Currency, Micros};
use crate::{Entity, Table};

/// A stretch of the client's time and the provider groups it needs (none, one or two).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, ToSchema)]
pub struct Step {
    #[schema(value_type = String, format = Duration, example = "PT15M")]
    pub duration: Duration,
    #[validate(length(min = 1))]
    pub provider1: Option<String>,
    #[validate(length(min = 1))]
    pub provider2: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "services", keys = ["tenant_id", "id"])]
#[schema(example = json!({"name": "Tooth extraction", "setup": "PT10M", "cleanup": "PT10M", "steps": [{"duration": "PT15M", "provider1": "hygienist"}, {"duration": "PT45M", "provider1": "hygienist", "provider2": "dentist"}, {"duration": "PT15M", "provider1": "hygienist"}]}))]
pub struct ServiceData {
    pub tenant_id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Tooth extraction")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = String, format = Duration, example = "PT10M")]
    pub setup: Duration,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = String, format = Duration, example = "PT10M")]
    pub cleanup: Duration,
    #[validate(length(min = 1), nested)]
    #[sqlx(json)]
    pub steps: Vec<Step>,
    /// What one of quantity counts; a session unless given.
    #[serde(default = "session")]
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "session")]
    pub unit: Name,
    /// The current price of one unit, in millionths of `currency`.
    #[schema(value_type = i64, example = 150000000)]
    pub price: Micros,
    #[schema(value_type = String, example = "USD")]
    pub currency: Currency,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Service = Entity<Uuid, ServiceData>;

fn session() -> Name {
    Name::try_new("session").expect("a name")
}

impl Priced for ServiceData {
    const KIND: ItemKind = ItemKind::Service;

    fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    fn price(&self) -> (Micros, Currency) {
        (self.price, self.currency.clone())
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/services",
    tag = "services",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, ServiceData>>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Service>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/services/{id}",
    tag = "services",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ServiceData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Service> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/services",
    tag = "services",
    params(("tenant_id" = Uuid, Path)),
    request_body = ServiceData,
    responses((status = 200, body = inline(Entity<Uuid, ServiceData>))),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<ServiceData>,
) -> ApiResult<Service> {
    price::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/services/{id}",
    tag = "services",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ServiceData,
    responses((status = 200, body = inline(Entity<Uuid, ServiceData>)), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Keyed<ServiceData>>,
) -> ApiResult<Service> {
    price::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/services/{id}",
    tag = "services",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = ServiceData, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Entity<Uuid, ServiceData>)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Service> {
    price::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/services/{id}",
    tag = "services",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ServiceData>)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Service> {
    crud::delete(&ctx.pool, &key).await
}
