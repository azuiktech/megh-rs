//! Schedule: when appointments can be booked, like NexHealth working hours.

use axum::extract::State;
use chrono::NaiveTime;
use rrule::RRuleSet;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::Array;
use validator::{Validate, ValidationError};

use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};

/// Open time: every day the recurrence gives, from `starts_at` to `ends_at`.
#[crate::entity(table = "schedules", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema)]
#[validate(schema(function = "ends_after_start"))]
#[schema(example = json!({"name": "Clinic hours", "recurrence": "DTSTART;TZID=Asia/Kolkata:20261023T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261230T235959Z", "starts_at": "09:00:00", "ends_at": "16:00:00"}))]
pub struct Schedule {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "Clinic hours")]
    pub name: Name,
    /// None: any provider.
    pub provider_id: Option<Uuid>,
    /// None: any resource.
    pub resource_id: Option<Uuid>,
    /// Empty: any service.
    #[serde(default)]
    #[sqlx(try_from = "Array<Uuid>")]
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
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

fn ends_after_start(schedule: &Schedule) -> Result<(), ValidationError> {
    (schedule.starts_at < schedule.ends_at)
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
    path = "/tenants/{tenant_id}/schedules",
    tag = "schedules",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Schedule>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Schedule>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/schedules/{id}",
    tag = "schedules",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Schedule)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Schedule> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/schedules",
    tag = "schedules",
    params(("tenant_id" = Uuid, Path)),
    request_body = Schedule,
    responses((status = 200, body = inline(Schedule))),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<Schedule>,
) -> ApiResult<Schedule> {
    crud::create(&ctx.pool, data, None).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/schedules/{id}",
    tag = "schedules",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Schedule,
    responses((status = 200, body = inline(Schedule)), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Schedule>,
) -> ApiResult<Schedule> {
    crud::replace(&ctx.pool, body, None).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/schedules/{id}",
    tag = "schedules",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = Schedule, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Schedule)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Schedule> {
    crud::patch(&ctx.pool, body, None).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/schedules/{id}",
    tag = "schedules",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Schedule)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Schedule> {
    crud::delete(&ctx.pool, &key, None).await
}
