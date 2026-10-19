//! `/api/v1/orgs/{org}/schedules` and `/blockouts`: open and blocked time, the same entity of two kinds.

use axum::extract::State;
use axum_params::Params;
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use validator::ValidateArgs;

use super::common::scope;
use super::page::paginate;
use super::AppContext;
use crate::auth::api::{ApiError, ApiResult, ErrorBody, ListQuery, Paginated};
use crate::booking::{Kind, Schedule, ScheduleId, ScheduleRow};
use crate::TableEntity;

#[derive(Deserialize)]
pub(super) struct ListSchedules {
    org: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct CreateSchedule {
    org: Uuid,
    #[serde(flatten)]
    schedule: Schedule,
}

#[derive(Deserialize)]
pub(super) struct ScheduleRef {
    org: Uuid,
    id: Uuid,
}

#[derive(Deserialize)]
pub(super) struct ReplaceSchedule {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    schedule: Schedule,
}

#[derive(Deserialize)]
pub(super) struct PatchSchedule {
    org: Uuid,
    id: Uuid,
    #[serde(flatten)]
    changes: serde_json::Value,
}

/// The two collections of the entity: the kind of schedule each holds.
trait Collection {
    const KIND: Kind;
}

struct Schedules;
struct Blockouts;

impl Collection for Schedules {
    const KIND: Kind = Kind::Open;
}

impl Collection for Blockouts {
    const KIND: Kind = Kind::Blockout;
}

/// The bookable or service of an axis, when given, belongs to the covered orgs.
async fn on_axis(pool: &PgPool, scope: &[Uuid], table: &str, id: Option<Uuid>) -> Result<(), ApiError> {
    let outside = || ApiError::invalid("an org, bookable or service of another org cannot be used");
    let Some(id) = id else { return Ok(()) };
    let owner: Option<Uuid> = sqlx::query_scalar(&format!("SELECT org_id FROM {table} WHERE id = $1 AND deleted_at IS NULL")).bind(id).fetch_optional(pool).await?;
    owner.filter(|owner| scope.contains(owner)).map(drop).ok_or_else(outside)
}

async fn check_axes(pool: &PgPool, scope: &[Uuid], schedule: &Schedule) -> Result<(), ApiError> {
    on_axis(pool, scope, "bookables", schedule.bookable_id.map(|id| id.0)).await?;
    on_axis(pool, scope, "services", schedule.service_id.map(|id| id.0)).await
}

fn of_kind<C: Collection>(schedule: &Schedule) -> Result<(), ApiError> {
    match schedule.kind == C::KIND {
        true => Ok(()),
        false => Err(ApiError::invalid(format!("kind must be {:?} in this collection", C::KIND))),
    }
}

async fn found<C: Collection>(pool: &PgPool, scope: &[Uuid], id: Uuid) -> ApiResult<ScheduleRow> {
    let row = sqlx::query_as("SELECT * FROM schedules WHERE id = $1 AND kind = $2 AND org_id = ANY($3)").bind(id).bind(C::KIND).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("schedule {id} not found")))
}

async fn live<C: Collection>(pool: &PgPool, scope: &[Uuid], id: Uuid) -> ApiResult<ScheduleRow> {
    let row = found::<C>(pool, scope, id).await?;
    match row.deleted_at {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("schedule {id} is deleted"))),
    }
}

async fn checked(pool: &PgPool, scope: &[Uuid], schedule: &Schedule) -> Result<(), ApiError> {
    schedule.validate_with_args(&scope.to_vec())?;
    check_axes(pool, scope, schedule).await
}

async fn list_in<C: Collection>(ctx: &AppContext, input: ListSchedules) -> ApiResult<Paginated<ScheduleRow>> {
    let scope = scope(&ctx.pool, input.org).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let sql = format!("SELECT * FROM schedules WHERE kind = $1 AND org_id = ANY($2){shown} ORDER BY created_at, id");
    let rows = sqlx::query_as(&sql).bind(C::KIND).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

async fn create_in<C: Collection>(ctx: &AppContext, input: CreateSchedule) -> ApiResult<ScheduleRow> {
    of_kind::<C>(&input.schedule)?;
    checked(&ctx.pool, &scope(&ctx.pool, input.org).await?, &input.schedule).await?;
    Ok(ScheduleRow::new(ScheduleId(Uuid::new_v4()), input.schedule).insert(&ctx.pool).await?)
}

async fn read_in<C: Collection>(ctx: &AppContext, input: ScheduleRef) -> ApiResult<ScheduleRow> {
    found::<C>(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await
}

async fn replace_in<C: Collection>(ctx: &AppContext, input: ReplaceSchedule) -> ApiResult<ScheduleRow> {
    of_kind::<C>(&input.schedule)?;
    let scope = scope(&ctx.pool, input.org).await?;
    let current = live::<C>(&ctx.pool, &scope, input.id).await?;
    checked(&ctx.pool, &scope, &input.schedule).await?;
    Ok(ScheduleRow::with_timestamps(current.id, input.schedule, current.created_at, Utc::now()).save(&ctx.pool).await?)
}

async fn patch_in<C: Collection>(ctx: &AppContext, input: PatchSchedule) -> ApiResult<ScheduleRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let patched = live::<C>(&ctx.pool, &scope, input.id).await?.patch(&input.changes).map_err(|error| ApiError::invalid(error.to_string()))?;
    of_kind::<C>(&patched)?;
    checked(&ctx.pool, &scope, &patched).await?;
    Ok(patched.save(&ctx.pool).await?)
}

async fn remove_in<C: Collection>(ctx: &AppContext, input: ScheduleRef) -> ApiResult<ScheduleRow> {
    let current = live::<C>(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await?;
    current.soft_delete(&ctx.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))
}

/// The six standard methods of a collection, documented under its own path and tag.
macro_rules! collection {
    ($module:ident, $kind:ident, $path:literal, $tag:literal) => {
        pub(super) mod $module {
            use super::*;

            #[utoipa::path(get, path = concat!("/api/v1/orgs/{org}/", $path), tag = $tag, params(("org" = Uuid, Path), ListQuery),
                responses((status = OK, body = Paginated<ScheduleRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
            pub(in super::super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListSchedules>) -> ApiResult<Paginated<ScheduleRow>> {
                list_in::<$kind>(&ctx, input).await
            }

            #[utoipa::path(post, path = concat!("/api/v1/orgs/{org}/", $path), tag = $tag, params(("org" = Uuid, Path)), request_body = Schedule,
                responses((status = OK, body = ScheduleRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
            pub(in super::super) async fn create(State(ctx): State<AppContext>, Params(input, _): Params<CreateSchedule>) -> ApiResult<ScheduleRow> {
                create_in::<$kind>(&ctx, input).await
            }

            #[utoipa::path(get, path = concat!("/api/v1/orgs/{org}/", $path, "/{id}"), tag = $tag, params(("org" = Uuid, Path), ("id" = Uuid, Path)),
                responses((status = OK, body = ScheduleRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
            pub(in super::super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<ScheduleRef>) -> ApiResult<ScheduleRow> {
                read_in::<$kind>(&ctx, input).await
            }

            #[utoipa::path(put, path = concat!("/api/v1/orgs/{org}/", $path, "/{id}"), tag = $tag, params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Schedule,
                responses((status = OK, body = ScheduleRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
            pub(in super::super) async fn replace(State(ctx): State<AppContext>, Params(input, _): Params<ReplaceSchedule>) -> ApiResult<ScheduleRow> {
                replace_in::<$kind>(&ctx, input).await
            }

            #[utoipa::path(patch, path = concat!("/api/v1/orgs/{org}/", $path, "/{id}"), tag = $tag, params(("org" = Uuid, Path), ("id" = Uuid, Path)), request_body = Schedule,
                responses((status = OK, body = ScheduleRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
            pub(in super::super) async fn patch(State(ctx): State<AppContext>, Params(input, _): Params<PatchSchedule>) -> ApiResult<ScheduleRow> {
                patch_in::<$kind>(&ctx, input).await
            }

            #[utoipa::path(delete, path = concat!("/api/v1/orgs/{org}/", $path, "/{id}"), tag = $tag, params(("org" = Uuid, Path), ("id" = Uuid, Path)),
                responses((status = OK, body = ScheduleRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
            pub(in super::super) async fn remove(State(ctx): State<AppContext>, Params(input, _): Params<ScheduleRef>) -> ApiResult<ScheduleRow> {
                remove_in::<$kind>(&ctx, input).await
            }
        }
    };
}

collection!(schedules, Schedules, "schedules", "schedules");
collection!(blockouts, Blockouts, "blockouts", "blockouts");
