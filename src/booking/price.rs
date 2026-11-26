//! Price: the ledger of what a product or service cost and from when. The item keeps its current price; each
//! create, and each change of price or currency, adds a row in the same transaction. Nobody writes the ledger directly.

use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::{Db, Tx};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use crate::auth::crud::{self, Row};
use crate::auth::rest::{
    ApiError, ApiResult, AppContext, Changes, Input, Key, ListRequest, Page,
};
use crate::billing::value::{Currency, Micros};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ItemKind {
    Product,
    Service,
}

#[crate::entity(table = "prices", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(
    Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema,
)]
pub struct Price {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    pub item_id: Uuid,
    pub item_kind: ItemKind,
    pub price: Micros,
    pub currency: Currency,
    pub effective_at: DateTime<Utc>,
}

/// An item with a current price that the ledger follows.
pub trait Priced: Row {
    const KIND: ItemKind;
    fn tenant_id(&self) -> Uuid;
    fn id(&self) -> Uuid;
    fn price(&self) -> (Micros, Currency);
}

pub(super) async fn create<T: Priced>(pool: &Db, data: T) -> ApiResult<T> {
    let mut transaction = pool.begin().await?;
    let saved = crud::create(&mut transaction, data, None).await?;
    record(&mut transaction, None, &saved).await?;
    transaction.commit().await?;
    Ok(saved)
}

pub(super) async fn replace<T: Priced>(pool: &Db, data: T) -> ApiResult<T> {
    let mut transaction = pool.begin().await?;
    let key = Key {
        tenant_id: data.tenant_id(),
        id: data.id(),
    };
    let before = locked_price::<T>(&mut transaction, &key).await?;
    let saved = crud::replace(&mut transaction, data, None).await?;
    record(&mut transaction, Some(before), &saved).await?;
    transaction.commit().await?;
    Ok(saved)
}

pub(super) async fn patch<T: Priced>(pool: &Db, body: Changes) -> ApiResult<T> {
    let mut transaction = pool.begin().await?;
    let before = locked_price::<T>(&mut transaction, &body.key).await?;
    let saved = crud::patch::<T>(&mut transaction, body, None).await?;
    record(&mut transaction, Some(before), &saved).await?;
    transaction.commit().await?;
    Ok(saved)
}

/// The item's price, its row locked until the transaction ends, so changes of one item are recorded in turn.
async fn locked_price<T: Priced>(
    transaction: &mut Tx,
    key: &Key,
) -> ApiResult<(Micros, Currency)> {
    let sql = format!(
        "SELECT * FROM {} WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL FOR UPDATE",
        T::TABLE_NAME
    );
    Db::query_as::<T>(&sql).sqlite(sql.replace(" FOR UPDATE", ""))
        .bind(key.tenant_id)
        .bind(key.id)
        .fetch_optional(&mut *transaction)
        .await?
        .map(|item| item.price())
        .ok_or(ApiError::NotFound)
}

/// Adds the item's price to the ledger unless it is the price it had before.
async fn record<T: Priced>(
    transaction: &mut Tx,
    before: Option<(Micros, Currency)>,
    item: &T,
) -> ApiResult<()> {
    let (price, currency) = item.price();
    let unchanged = before == Some((price, currency.clone()));
    let row = Price {
        tenant_id: item.tenant_id(),
        item_id: item.id(),
        item_kind: T::KIND,
        price,
        currency,
        effective_at: Utc::now(),
        created_at: Utc::now(),
        updated_at: None,
        deleted_at: None,
        id: Uuid::now_v7(),
    };
    match unchanged {
        true => Ok(()),
        false => crud::create(&mut *transaction, row, None).await.map(drop),
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
    responses((status = 200, body = inline(Page<Price>))),
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
    responses((status = 200, body = inline(Price)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Price> {
    crud::get(&ctx.pool, &key).await
}
