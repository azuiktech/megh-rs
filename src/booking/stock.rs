//! Stock: how much of a product one tenant holds. Read only here; payments change it.

use axum::extract::State;
use axum::routing::get as get_route;
use serde::{Deserialize, Serialize};
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Input, Key, ListRequest, Page};
use crate::billing::value::Quantity;
use crate::{Entity, Table};

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema,
)]
#[table(name = "stocks", keys = ["tenant_id", "id"])]
pub struct StockData {
    /// The tenant holding it; the product may be another tenant's.
    pub tenant_id: Uuid,
    pub product_id: Uuid,
    #[schema(value_type = String, example = "4")]
    pub on_hand: Quantity,
}

pub type Stock = Entity<Uuid, StockData>;

/// The custom method on a product; the path segment is `:stock`.
#[derive(Debug, Clone, Copy, Deserialize)]
enum Verb {
    #[serde(rename = ":stock")]
    Stock,
}

#[derive(Debug, Deserialize, Validate)]
struct OfProduct {
    #[serde(flatten)]
    key: Key,
    #[serde(rename = "verb")]
    _verb: Verb,
}

#[derive(OpenApi)]
#[openapi(paths(of_product))]
struct Custom;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Custom::openapi())
        .routes(routes!(list))
        .routes(routes!(get))
        .route(
            "/tenants/{tenant_id}/products/{id}/{verb}",
            get_route(of_product),
        )
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/stocks",
    tag = "stocks",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity. Example: `product_id = \"…\"`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, StockData>>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Stock>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/stocks/{id}",
    tag = "stocks",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, StockData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Stock> {
    crud::get(&ctx.pool, &key).await
}

/// `:stock`: the tenant's stock of the product.
#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/products/{id}:stock",
    tag = "products",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path, description = "The product")),
    responses((status = 200, body = inline(Entity<Uuid, StockData>)), (status = 404, description = "The tenant holds none of it.")),
)]
async fn of_product(
    State(ctx): State<AppContext>,
    Input(OfProduct { key, .. }): Input<OfProduct>,
) -> ApiResult<Stock> {
    sqlx::query_as(
        "SELECT * FROM stocks WHERE tenant_id = $1 AND product_id = $2 AND deleted_at IS NULL",
    )
    .bind(key.tenant_id)
    .bind(key.id)
    .fetch_optional(&ctx.pool)
    .await?
    .ok_or(ApiError::NotFound)
}
