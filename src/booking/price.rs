//! Price: the ledger of what a product or service cost and from when. The item keeps its current price; each
//! create, and each change of price or currency, adds a row in the same transaction. Nobody writes the ledger directly.

use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, PgTransaction};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use crate::auth::crud::{self, Row};
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::value::{Currency, Micros};
use crate::{Entity, Table, TableEntity};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Product,
    Service,
}

#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema,
)]
#[table(name = "prices", keys = ["tenant_id", "id"])]
pub struct PriceData {
    pub tenant_id: Uuid,
    pub item_id: Uuid,
    pub item_kind: ItemKind,
    pub price: Micros,
    pub currency: Currency,
    pub effective_at: DateTime<Utc>,
}

pub type Price = Entity<Uuid, PriceData>;

/// An item with a current price that the ledger follows.
pub trait Priced: Row {
    const KIND: ItemKind;
    fn tenant_id(&self) -> Uuid;
    fn price(&self) -> (Micros, Currency);
}

pub(super) async fn create<T: Priced>(pool: &PgPool, data: T) -> ApiResult<Entity<Uuid, T>> {
    let mut transaction = pool.begin().await?;
    let saved = crud::create(&mut *transaction, data).await?;
    record(&mut transaction, None, &saved).await?;
    transaction.commit().await?;
    Ok(saved)
}

pub(super) async fn replace<T: Priced>(
    pool: &PgPool,
    body: Keyed<T>,
) -> ApiResult<Entity<Uuid, T>> {
    let mut transaction = pool.begin().await?;
    let key = Key {
        tenant_id: body.data.tenant_id(),
        id: body.id,
    };
    let before = crud::get::<T>(&mut *transaction, &key).await?.data.price();
    let saved = crud::replace(&mut *transaction, body).await?;
    record(&mut transaction, Some(before), &saved).await?;
    transaction.commit().await?;
    Ok(saved)
}

pub(super) async fn patch<T: Priced>(pool: &PgPool, body: Changes) -> ApiResult<Entity<Uuid, T>> {
    let mut transaction = pool.begin().await?;
    let before = crud::get::<T>(&mut *transaction, &body.key)
        .await?
        .data
        .price();
    let saved = crud::patch::<T>(&mut *transaction, body).await?;
    record(&mut transaction, Some(before), &saved).await?;
    transaction.commit().await?;
    Ok(saved)
}

/// Adds the item's price to the ledger unless it is the price it had before.
async fn record<T: Priced>(
    transaction: &mut PgTransaction<'_>,
    before: Option<(Micros, Currency)>,
    item: &Entity<Uuid, T>,
) -> ApiResult<()> {
    let (price, currency) = item.data.price();
    let unchanged = before == Some((price, currency.clone()));
    let row = PriceData {
        tenant_id: item.data.tenant_id(),
        item_id: item.id,
        item_kind: T::KIND,
        price,
        currency,
        effective_at: Utc::now(),
    };
    match unchanged {
        true => Ok(()),
        false => TableEntity::insert(&Entity::new(Uuid::now_v7(), row), &mut **transaction)
            .await
            .map(drop)
            .map_err(Into::into),
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list))
        .routes(routes!(get))
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/prices",
    tag = "prices",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity. Example: `item_id = \"…\"`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, PriceData>>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Price>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/prices/{id}",
    tag = "prices",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, PriceData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Price> {
    crud::get(&ctx.pool, &key).await
}
