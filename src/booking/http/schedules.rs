//! `/api/v1/org/{org_id}/schedules` (times open for work) and `.../blockouts` (closed times, with a reason).

use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use axum_extra::extract::{Query, WithRejection};
use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::error::ApiError;
use super::page::{page, ListQuery};
use super::{subtree, BookingState};
use crate::booking::{BookableId, Kind, OrgId, Recurrence, Schedule, ScheduleId, ScheduleRow, ServiceId};
use crate::TableEntity;

#[derive(Clone, Copy)]
struct Collection {
    kind: Kind,
    name: &'static str,
}

/// The fields of a schedule or blockout. The time is either `recurrence` (RFC 5545, `DTSTART;TZID=...`) with a `span`,
/// or the shorthand `days` or `date` with `start_time` and `end_time`, read in the org's time zone.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScheduleBody {
    org_id: Option<OrgId>,
    service_id: Option<ServiceId>,
    bookable_id: Option<BookableId>,
    recurrence: Option<String>,
    span: Option<String>,
    days: Option<Vec<String>>,
    date: Option<NaiveDate>,
    start_date: Option<NaiveDate>,
    start_time: Option<String>,
    end_time: Option<String>,
    reason: Option<String>,
    notes: Option<String>,
}

#[derive(Serialize)]
struct ScheduleResource {
    name: String,
    id: ScheduleId,
    org_id: Option<OrgId>,
    service_id: Option<ServiceId>,
    bookable_id: Option<BookableId>,
    recurrence: String,
    span: String,
    reason: Option<String>,
    notes: Option<String>,
    create_time: DateTime<Tz>,
    update_time: DateTime<Tz>,
    delete_time: Option<DateTime<Tz>>,
}

#[derive(Debug, Default, Deserialize)]
struct UpdateQuery {
    update_mask: Option<String>,
}

pub(super) fn routes(name: &'static str, kind: Kind) -> Router<BookingState> {
    let collection = format!("/api/v1/org/{{org_id}}/{name}");
    let one = format!("{collection}/{{id}}");
    Router::new()
        .route(&collection, get(list).post(create))
        .route(&one, get(read).patch(update).delete(remove))
        .layer(Extension(Collection { kind, name }))
}

/// `3600s`, with a fraction when needed (AIP-142).
fn seconds(duration: Duration) -> String {
    match duration.subsec_nanos() {
        0 => format!("{}s", duration.as_secs()),
        nanos => format!("{}.{:09}s", duration.as_secs(), nanos).trim_end_matches('0').to_string() + "s",
    }
}

fn resource(row: &ScheduleRow, collection: Collection, org: Uuid, zone: Tz) -> ScheduleResource {
    let local = |time: DateTime<Utc>| time.with_timezone(&zone);
    ScheduleResource {
        name: format!("org/{org}/{}/{}", collection.name, row.id.0),
        id: row.id,
        org_id: row.org_id,
        service_id: row.service_id,
        bookable_id: row.bookable_id,
        recurrence: row.recurrence.0.clone(),
        span: seconds(row.span),
        reason: row.reason.clone(),
        notes: row.notes.clone(),
        create_time: local(row.created_at),
        update_time: local(row.updated_at),
        delete_time: row.deleted_at.map(local),
    }
}

async fn zone_of(pool: &PgPool, org: Uuid) -> Result<Tz, ApiError> {
    let found: Option<Option<String>> = sqlx::query_scalar("SELECT timezone FROM organizations WHERE id = $1").bind(org).fetch_optional(pool).await?;
    let name = found.ok_or_else(|| ApiError::not_found(format!("org {org} not found")))?;
    name.map_or(Ok(Tz::UTC), |name| name.parse().map_err(|_| ApiError::invalid(format!("the time zone {name} of the org is not known"))))
}

fn clock(text: &str) -> Result<NaiveTime, ApiError> {
    NaiveTime::parse_from_str(text, "%H:%M").or_else(|_| NaiveTime::parse_from_str(text, "%H:%M:%S")).map_err(|_| ApiError::invalid(format!("{text:?} is not a time like 15:00")))
}

fn span_of(text: &str) -> Result<Duration, ApiError> {
    humantime::parse_duration(text).map_err(|_| ApiError::invalid(format!("{text:?} is not a duration like 3600s")))
}

fn shorthand(body: &ScheduleBody) -> bool {
    body.days.is_some() || body.date.is_some() || body.start_time.is_some() || body.end_time.is_some() || body.start_date.is_some()
}

fn weekdays(days: &[String]) -> Result<String, ApiError> {
    let known = ["MO", "TU", "WE", "TH", "FR", "SA", "SU"];
    let upper: Vec<String> = days.iter().map(|day| day.to_uppercase()).collect();
    match upper.iter().find(|day| !known.contains(&day.as_str())) {
        Some(day) => Err(ApiError::invalid(format!("{day:?} is not a weekday, use MO TU WE TH FR SA SU"))),
        None => Ok(upper.join(",")),
    }
}

/// The recurrence and span of the shorthand, in the org's time zone.
fn from_shorthand(body: &ScheduleBody, zone: Tz) -> Result<(Recurrence, Duration), ApiError> {
    let (Some(start), Some(end)) = (&body.start_time, &body.end_time) else { return Err(ApiError::invalid("start_time and end_time are required")) };
    let (start, end) = (clock(start)?, clock(end)?);
    let span = (end - start).to_std().ok().filter(|span| !span.is_zero()).ok_or_else(|| ApiError::invalid("end_time must be after start_time"))?;
    let first = body.date.or(body.start_date).unwrap_or_else(|| Utc::now().with_timezone(&zone).date_naive());
    let rule = match (&body.days, body.date) {
        (Some(days), None) => format!("RRULE:FREQ=WEEKLY;BYDAY={}", weekdays(days)?),
        (None, Some(_)) => "RRULE:FREQ=DAILY;COUNT=1".to_string(),
        _ => return Err(ApiError::invalid("give either days or date")),
    };
    let begin = format!("DTSTART;TZID={}:{}T{}", zone.name(), first.format("%Y%m%d"), start.format("%H%M%S"));
    Ok((Recurrence(format!("{begin}\n{rule}")), span))
}

fn timing(body: &ScheduleBody, zone: Tz) -> Result<(Recurrence, Duration), ApiError> {
    let (recurrence, span) = match (&body.recurrence, &body.span) {
        (Some(recurrence), Some(span)) if !shorthand(body) => (Recurrence(recurrence.clone()), span_of(span)?),
        _ => from_shorthand(body, zone)?,
    };
    recurrence.validate()?;
    Ok((recurrence, span))
}

/// The org axis, the bookable and the service must belong to the org of the path or an org grouped under it.
async fn check_axes(pool: &PgPool, org: Uuid, schedule: &Schedule) -> Result<(), ApiError> {
    let scope = subtree(pool, org).await?;
    let outside = || ApiError::invalid("an org, bookable or service of another org cannot be used");
    if schedule.org_id.is_some_and(|axis| !scope.contains(&axis)) {
        return Err(outside());
    }
    if let Some(bookable) = schedule.bookable_id {
        let owner: Option<Uuid> = sqlx::query_scalar("SELECT org_id FROM bookables WHERE id = $1 AND deleted_at IS NULL").bind(bookable).fetch_optional(pool).await?;
        owner.filter(|owner| scope.contains(owner)).ok_or_else(outside)?;
    }
    if let Some(service) = schedule.service_id {
        let found: Option<Option<Uuid>> = sqlx::query_scalar("SELECT org_id FROM services WHERE id = $1 AND deleted_at IS NULL").bind(service).fetch_optional(pool).await?;
        let owner = found.ok_or_else(outside)?;
        if owner.is_some_and(|owner| !scope.contains(&owner)) {
            return Err(outside());
        }
    }
    Ok(())
}

async fn fetch(pool: &PgPool, collection: Collection, org: Uuid, id: ScheduleId) -> Result<ScheduleRow, ApiError> {
    let scope = subtree(pool, org).await?;
    let query = "SELECT * FROM schedules WHERE id = $1 AND kind = $2 AND org_id = ANY($3)";
    let row = sqlx::query_as(query).bind(id).bind(collection.kind).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("{} {} not found", collection.name, id.0)))
}

async fn live(pool: &PgPool, collection: Collection, org: Uuid, id: ScheduleId) -> Result<ScheduleRow, ApiError> {
    let row = fetch(pool, collection, org, id).await?;
    match row.deleted_at {
        None => Ok(row),
        Some(_) => Err(ApiError::not_found(format!("{} {} is deleted", collection.name, id.0))),
    }
}

async fn create(
    State(state): State<BookingState>,
    Extension(collection): Extension<Collection>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<ScheduleBody>, ApiError>,
) -> Result<Json<ScheduleResource>, ApiError> {
    let zone = zone_of(&state.pool, org).await?;
    let (recurrence, span) = timing(&body, zone)?;
    let data = Schedule {
        org_id: Some(body.org_id.unwrap_or(org)),
        service_id: body.service_id,
        bookable_id: body.bookable_id,
        kind: collection.kind,
        recurrence,
        span,
        reason: body.reason,
        notes: body.notes,
    };
    check_axes(&state.pool, org, &data).await?;
    let row = ScheduleRow::new(ScheduleId(Uuid::new_v4()), data).insert(&state.pool).await?;
    Ok(Json(resource(&row, collection, org, zone)))
}

async fn read(
    State(state): State<BookingState>,
    Extension(collection): Extension<Collection>,
    WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, ScheduleId)>, ApiError>,
) -> Result<Json<ScheduleResource>, ApiError> {
    let zone = zone_of(&state.pool, org).await?;
    Ok(Json(resource(&fetch(&state.pool, collection, org, id).await?, collection, org, zone)))
}

async fn list(
    State(state): State<BookingState>,
    Extension(collection): Extension<Collection>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Query(query), _): WithRejection<Query<ListQuery>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let zone = zone_of(&state.pool, org).await?;
    let scope = subtree(&state.pool, org).await?;
    let shown = if query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let sql = format!("SELECT * FROM schedules WHERE kind = $1 AND org_id = ANY($2){shown} ORDER BY created_at, id");
    let rows: Vec<ScheduleRow> = sqlx::query_as(&sql).bind(collection.kind).bind(scope).fetch_all(&state.pool).await?;
    let items = rows.iter().map(|row| serde_json::to_value(resource(row, collection, org, zone)).map_err(|e| ApiError::invalid(e.to_string()))).collect::<Result<_, _>>()?;
    Ok(Json(page(collection.name, items, &query)?))
}

/// Whether a field is updated: listed in the `update_mask`, or, without a mask, given in the body.
fn updates(mask: &Option<Vec<String>>, field: &str, given: bool) -> bool {
    mask.as_ref().map_or(given, |fields| fields.iter().any(|name| name == field))
}

fn changed(current: &Schedule, body: ScheduleBody, mask: &Option<Vec<String>>, org: Uuid, zone: Tz) -> Result<Schedule, ApiError> {
    let retime = updates(mask, "recurrence", body.recurrence.is_some() || shorthand(&body)) || updates(mask, "span", body.span.is_some());
    let (recurrence, span) = match (retime, shorthand(&body)) {
        (false, _) => (current.recurrence.clone(), current.span),
        (true, true) => timing(&body, zone)?,
        (true, false) => {
            let recurrence = body.recurrence.clone().map_or_else(|| current.recurrence.clone(), Recurrence);
            recurrence.validate()?;
            (recurrence, body.span.as_deref().map(span_of).transpose()?.unwrap_or(current.span))
        }
    };
    Ok(Schedule {
        org_id: if updates(mask, "org_id", body.org_id.is_some()) { Some(body.org_id.unwrap_or(org)) } else { current.org_id },
        service_id: if updates(mask, "service_id", body.service_id.is_some()) { body.service_id } else { current.service_id },
        bookable_id: if updates(mask, "bookable_id", body.bookable_id.is_some()) { body.bookable_id } else { current.bookable_id },
        kind: current.kind,
        recurrence,
        span,
        reason: if updates(mask, "reason", body.reason.is_some()) { body.reason } else { current.reason.clone() },
        notes: if updates(mask, "notes", body.notes.is_some()) { body.notes } else { current.notes.clone() },
    })
}

async fn update(
    State(state): State<BookingState>,
    Extension(collection): Extension<Collection>,
    WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, ScheduleId)>, ApiError>,
    WithRejection(Query(query), _): WithRejection<Query<UpdateQuery>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<ScheduleBody>, ApiError>,
) -> Result<Json<ScheduleResource>, ApiError> {
    let zone = zone_of(&state.pool, org).await?;
    let current = live(&state.pool, collection, org, id).await?;
    let mask = query.update_mask.map(|mask| mask.split(',').map(|field| field.trim().to_string()).collect::<Vec<_>>());
    let data = changed(&current, body, &mask, org, zone)?;
    check_axes(&state.pool, org, &data).await?;
    let saved = ScheduleRow::with_timestamps(id, data, current.created_at, Utc::now()).save(&state.pool).await?;
    sqlx::query("UPDATE schedules SET updated_at = now() WHERE id = $1").bind(id).execute(&state.pool).await?;
    Ok(Json(resource(&fetch(&state.pool, collection, org, saved.id).await?, collection, org, zone)))
}

async fn remove(
    State(state): State<BookingState>,
    Extension(collection): Extension<Collection>,
    WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, ScheduleId)>, ApiError>,
) -> Result<Json<ScheduleResource>, ApiError> {
    let zone = zone_of(&state.pool, org).await?;
    let current = live(&state.pool, collection, org, id).await?;
    let deleted = current.soft_delete(&state.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))?;
    Ok(Json(resource(&deleted, collection, org, zone)))
}
