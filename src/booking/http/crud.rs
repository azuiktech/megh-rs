//! The standard methods of Google AIP-131 to 135 and 164 for any collection of rows owned by an org: Get, List,
//! Create, Update (`PATCH` with `update_mask`) and Delete (soft; the resource comes back with `delete_time`).
//!
//! A collection says how its rows look and change ([`Collection`]); this module routes, scopes by org, pages and
//! filters. Persistence is a few concrete calls per collection because the futures of a generic `TableEntity` are
//! not `Send`.

use std::future::Future;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::{Query, WithRejection};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{Map, Value};
use sqlx::postgres::PgRow;
use sqlx::PgPool;
use uuid::Uuid;

use super::common::zone_of;
use crate::auth::api::ApiError;
use super::page::page;
use crate::auth::api::ListQuery;
use super::{subtree, AppContext};

/// The org of the path, its time zone and the orgs it covers (itself and those grouped under it).
pub(super) struct Context {
    pub(super) pool: PgPool,
    pub(super) org: Uuid,
    pub(super) zone: Tz,
    pub(super) scope: Vec<Uuid>,
}

pub(super) struct Stamps {
    pub(super) id: Uuid,
    pub(super) created: DateTime<Utc>,
    pub(super) updated: DateTime<Utc>,
    pub(super) deleted: Option<DateTime<Utc>>,
}

/// The fields named by `update_mask`, when it was given.
pub(super) type Mask = Option<Vec<String>>;

/// Whether a field is updated: listed in the mask, or, without a mask, given in the body.
pub(super) fn updates(mask: &Mask, field: &str, given: bool) -> bool {
    mask.as_ref().map_or(given, |fields| fields.iter().any(|name| name == field))
}

pub(super) trait Collection: 'static {
    type Row: Send + Sync + Unpin + for<'r> sqlx::FromRow<'r, PgRow>;
    type Body: DeserializeOwned + Send + 'static;
    /// The path segment, in the lowerCamelCase of AIP-122, such as `bookables`.
    const NAME: &'static str;
    const TABLE: &'static str;
    /// The order of a list, a SQL `ORDER BY` list.
    const ORDER: &'static str = "created_at, id";

    fn stamps(row: &Self::Row) -> Stamps;
    /// The fields of the resource other than `name`, `id` and the times.
    fn fields(row: &Self::Row, ctx: &Context) -> Value;
    fn create(ctx: &Context, body: Self::Body) -> impl Future<Output = Result<Self::Row, ApiError>> + Send;
    /// The row after the update: same id and `created_at`, new data.
    fn update(ctx: &Context, current: &Self::Row, body: Self::Body, mask: &Mask) -> impl Future<Output = Result<Self::Row, ApiError>> + Send;
    fn insert(pool: &PgPool, row: Self::Row) -> impl Future<Output = Result<Self::Row, sqlx::Error>> + Send;
    fn save(pool: &PgPool, row: Self::Row) -> impl Future<Output = Result<Self::Row, sqlx::Error>> + Send;
    fn delete(pool: &PgPool, row: Self::Row) -> impl Future<Output = Result<Option<Self::Row>, sqlx::Error>> + Send;
}

#[derive(Debug, Default, Deserialize)]
struct UpdateQuery {
    update_mask: Option<String>,
}

/// Get, List, Create, Update and Delete.
pub(super) fn routes<C: Collection>() -> Router<AppContext> {
    let collection = format!("/api/v1/orgs/{{org_id}}/{}", C::NAME);
    Router::new().route(&collection, get(list::<C>).post(create::<C>)).route(&format!("{collection}/{{id}}"), get(read::<C>).patch(update::<C>).delete(remove::<C>))
}

/// Get, List, Create and Delete, for rows that are made and unmade, never changed.
pub(super) fn routes_unchangeable<C: Collection>() -> Router<AppContext> {
    let collection = format!("/api/v1/orgs/{{org_id}}/{}", C::NAME);
    Router::new().route(&collection, get(list::<C>).post(create::<C>)).route(&format!("{collection}/{{id}}"), get(read::<C>).delete(remove::<C>))
}

async fn context(state: &AppContext, org: Uuid) -> Result<Context, ApiError> {
    Ok(Context { pool: state.pool.clone(), org, zone: zone_of(&state.pool, org).await?, scope: subtree(&state.pool, org).await? })
}

fn resource<C: Collection>(row: &C::Row, ctx: &Context) -> Value {
    let (stamps, local) = (C::stamps(row), |time: DateTime<Utc>| time.with_timezone(&ctx.zone));
    let mut fields = match C::fields(row, ctx) {
        Value::Object(fields) => fields,
        _ => Map::new(),
    };
    fields.insert("name".into(), format!("orgs/{}/{}/{}", ctx.org, C::NAME, stamps.id).into());
    fields.insert("id".into(), stamps.id.to_string().into());
    fields.insert("create_time".into(), serde_json::to_value(local(stamps.created)).unwrap_or_default());
    fields.insert("update_time".into(), serde_json::to_value(local(stamps.updated)).unwrap_or_default());
    fields.insert("delete_time".into(), serde_json::to_value(stamps.deleted.map(local)).unwrap_or_default());
    Value::Object(fields)
}

async fn fetch<C: Collection>(ctx: &Context, id: Uuid) -> Result<C::Row, ApiError> {
    let sql = format!("SELECT * FROM {} WHERE id = $1 AND org_id = ANY($2)", C::TABLE);
    let row = sqlx::query_as::<_, C::Row>(&sql).bind(id).bind(&ctx.scope).fetch_optional(&ctx.pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("{} {id} not found", C::NAME)))
}

async fn live<C: Collection>(ctx: &Context, id: Uuid) -> Result<C::Row, ApiError> {
    let row = fetch::<C>(ctx, id).await?;
    match C::stamps(&row).deleted {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("{} {id} is deleted", C::NAME))),
    }
}

async fn create<C: Collection>(
    State(state): State<AppContext>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<C::Body>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let ctx = context(&state, org).await?;
    let row = C::create(&ctx, body).await?;
    Ok(Json(resource::<C>(&C::insert(&ctx.pool, row).await?, &ctx)))
}

async fn read<C: Collection>(State(state): State<AppContext>, WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, Uuid)>, ApiError>) -> Result<Json<Value>, ApiError> {
    let ctx = context(&state, org).await?;
    Ok(Json(resource::<C>(&fetch::<C>(&ctx, id).await?, &ctx)))
}

async fn list<C: Collection>(
    State(state): State<AppContext>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Query(query), _): WithRejection<Query<ListQuery>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let ctx = context(&state, org).await?;
    let shown = if query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let sql = format!("SELECT * FROM {} WHERE org_id = ANY($1){shown} ORDER BY {}", C::TABLE, C::ORDER);
    let rows = sqlx::query_as::<_, C::Row>(&sql).bind(&ctx.scope).fetch_all(&ctx.pool).await?;
    let items = rows.iter().map(|row| resource::<C>(row, &ctx)).collect();
    Ok(Json(page(C::NAME, items, &query)?))
}

async fn update<C: Collection>(
    State(state): State<AppContext>,
    WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, Uuid)>, ApiError>,
    WithRejection(Query(query), _): WithRejection<Query<UpdateQuery>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<C::Body>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let ctx = context(&state, org).await?;
    let current = live::<C>(&ctx, id).await?;
    let mask: Mask = query.update_mask.map(|mask| mask.split(',').map(|field| field.trim().to_string()).collect());
    let changed = C::update(&ctx, &current, body, &mask).await?;
    C::save(&ctx.pool, changed).await?;
    sqlx::query(&format!("UPDATE {} SET updated_at = now() WHERE id = $1", C::TABLE)).bind(id).execute(&ctx.pool).await?;
    Ok(Json(resource::<C>(&fetch::<C>(&ctx, id).await?, &ctx)))
}

async fn remove<C: Collection>(State(state): State<AppContext>, WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, Uuid)>, ApiError>) -> Result<Json<Value>, ApiError> {
    let ctx = context(&state, org).await?;
    let current = live::<C>(&ctx, id).await?;
    let deleted = C::delete(&ctx.pool, current).await?.ok_or_else(|| ApiError::not_found("already deleted"))?;
    Ok(Json(resource::<C>(&deleted, &ctx)))
}
