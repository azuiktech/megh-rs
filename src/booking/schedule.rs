//! Schedule: when appointments can be booked, like NexHealth working hours.

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

/// Open time: every day the recurrence gives, from `starts_at` to `ends_at`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "schedules", keys = ["org_id", "id"])]
#[validate(schema(function = "ends_after_start"))]
pub struct ScheduleData {
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
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Schedule = Entity<Uuid, ScheduleData>;

fn ends_after_start(schedule: &ScheduleData) -> Result<(), ValidationError> {
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
    path = "/orgs/{org_id}/schedules",
    tag = "schedules",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter"),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = Page<Entity<Uuid, ScheduleData>>)),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Schedule>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/schedules/{id}",
    tag = "schedules",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ScheduleData>), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Schedule> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/schedules",
    tag = "schedules",
    params(("org_id" = Uuid, Path)),
    request_body = ScheduleData,
    responses((status = 200, body = Entity<Uuid, ScheduleData>)),
)]
async fn create(
    State(ctx): State<AppContext>,
    Input(data): Input<ScheduleData>,
) -> ApiResult<Schedule> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/schedules/{id}",
    tag = "schedules",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ScheduleData,
    responses((status = 200, body = Entity<Uuid, ScheduleData>), (status = 404)),
)]
async fn replace(
    State(ctx): State<AppContext>,
    Input(body): Input<Keyed<ScheduleData>>,
) -> ApiResult<Schedule> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/schedules/{id}",
    tag = "schedules",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = Entity<Uuid, ScheduleData>), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Schedule> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/schedules/{id}",
    tag = "schedules",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, ScheduleData>), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Schedule> {
    crud::delete(&ctx.pool, &key).await
}
