//! Blockout: time taken out of the schedules, such as a holiday or a broken instrument.

use axum::extract::State;
use chrono::NaiveTime;
use rrule::RRuleSet;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::{Entity, Table};

/// Blocked time: every day the recurrence gives, from `starts_at` to `ends_at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "blockouts", keys = ["org_id", "id"])]
#[validate(schema(function = "ends_after_start"))]
#[schema(example = json!({"name": "Lunch", "recurrence": "DTSTART;TZID=Asia/Kolkata:20261023T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE", "starts_at": "13:00:00", "ends_at": "14:00:00", "reason": "lunch break"}))]
pub struct BlockoutData {
    pub org_id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Clinic hours")]
    pub name: Name,
    /// None: any provider.
    pub provider_id: Option<Uuid>,
    /// None: any resource.
    pub resource_id: Option<Uuid>,
    /// Empty: any service.
    #[serde(default)]
    pub service_ids: Vec<Uuid>,
    /// DTSTART is when the rule starts, UNTIL or COUNT when it ends.
    #[sqlx(json)]
    #[schema(value_type = String, example = "DTSTART;TZID=Asia/Kolkata:20261023T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261230T235959Z")]
    pub recurrence: RRuleSet,
    /// When each window starts, in DTSTART's time zone; a window ends the same day.
    #[schema(value_type = String, example = "09:00:00")]
    pub starts_at: NaiveTime,
    #[schema(value_type = String, example = "16:00:00")]
    pub ends_at: NaiveTime,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Blockout = Entity<Uuid, BlockoutData>;

fn ends_after_start(blockout: &BlockoutData) -> Result<(), ValidationError> {
    (blockout.starts_at < blockout.ends_at)
        .then_some(())
        .ok_or_else(|| ValidationError::new("ends_at must be after starts_at"))
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/blockouts",
    tag = "blockouts",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, BlockoutData>>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Blockout>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/blockouts/{id}",
    tag = "blockouts",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, BlockoutData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Blockout> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/blockouts",
    tag = "blockouts",
    params(("org_id" = Uuid, Path)),
    request_body = BlockoutData,
    responses((status = 200, body = inline(Entity<Uuid, BlockoutData>))),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<BlockoutData>,
) -> ApiResult<Blockout> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/blockouts/{id}",
    tag = "blockouts",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = BlockoutData,
    responses((status = 200, body = inline(Entity<Uuid, BlockoutData>)), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Keyed<BlockoutData>>,
) -> ApiResult<Blockout> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/blockouts/{id}",
    tag = "blockouts",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = BlockoutData, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Entity<Uuid, BlockoutData>)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Blockout> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/blockouts/{id}",
    tag = "blockouts",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, BlockoutData>)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Blockout> {
    crud::delete(&ctx.pool, &key).await
}
