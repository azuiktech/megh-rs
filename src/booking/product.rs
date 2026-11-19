//! Product: what is sold or used with a service, such as a hair colour or a tooth. One tenant owns it; any tenant
//! may hold stock of it. Its price changes go to the price ledger.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::price::{self, ItemKind, Priced};
use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};
use crate::billing::value::{Currency, Micros};

#[crate::entity(table = "products", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema,
)]
#[schema(example = json!({"sku": "WEL-60", "name": "Hair colour", "unit": "tube", "price": 12000000, "currency": "USD"}))]
pub struct Product {
    /// The owner.
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    /// Unique per owner.
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "WEL-60")]
    pub sku: Name,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Hair colour")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
    /// What one of quantity counts, such as tube, ml or piece.
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "tube")]
    pub unit: Name,
    /// The current price of one unit, in millionths of `currency`.
    #[schema(value_type = i64, example = 12000000)]
    pub price: Micros,
    #[schema(value_type = String, example = "USD")]
    pub currency: Currency,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

impl Priced for Product {
    const KIND: ItemKind = ItemKind::Product;

    fn tenant_id(&self) -> Uuid {
        self.tenant_id
    }

    fn id(&self) -> Uuid {
        self.id
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
    path = "/tenants/{tenant_id}/products",
    tag = "products",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `sku = \"WEL-60\"`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Product>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Product>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/products/{id}",
    tag = "products",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Product)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Product> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/products",
    tag = "products",
    params(("tenant_id" = Uuid, Path)),
    request_body = Product,
    responses((status = 200, body = inline(Product)), (status = 409, description = "The owner has a product with this SKU.")),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<Product>,
) -> ApiResult<Product> {
    price::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/products/{id}",
    tag = "products",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Product,
    responses((status = 200, body = inline(Product)), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Product>,
) -> ApiResult<Product> {
    price::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/products/{id}",
    tag = "products",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = Product, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Product)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Product> {
    price::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/products/{id}",
    tag = "products",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Product)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Product> {
    crud::delete(&ctx.pool, &key, None).await
}
