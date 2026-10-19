//! `/api/v1/orgs/{org_id}/appointments`: one client having one service in a room with zero or more providers. A slot
//! found by a search is booked as one appointment for each of its placements, directly or by holding it first.
//!
//! - `POST .../appointments:hold` keeps a slot for a while (`held`);
//! - `POST .../appointments` books a slot directly; `PATCH .../appointments/{id}` moves an appointment to a slot;
//!   `DELETE .../appointments/{id}` cancels it;
//! - `POST .../appointments/{id}:book` books a held or waitlisted appointment (a held one with the `payment`);
//!   `POST .../appointments/{id}:confirm` confirms a booked one. One route reads the `:verb`, because axum does not
//!   match a literal after a parameter.

use axum::extract::State;
use axum_params::Params;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::common::zone_of;
use super::page::paginate;
use super::slot::{claimed_placements, verified, SlotBody};
use super::{subtree, AppContext};
use crate::auth::api::{ApiError, ApiResult, ErrorBody, ListQuery, Paginated};
use crate::booking::{booking_print, AppointmentId, AppointmentRow, Appointments, IdempotencyKey, Payment, Reservations, Status};

const LONGEST_HOLD_HOURS: i64 = 24;

#[derive(Deserialize)]
pub(super) struct ListAppointments {
    org_id: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct AppointmentRef {
    org_id: Uuid,
    id: Uuid,
}

/// A slot to keep until `expires_at`.
#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct HoldSlot {
    #[schema(value_type = String, format = Uuid)]
    org_id: Uuid,
    slot: SlotBody,
    expires_at: DateTime<Utc>,
}

/// A slot to book, with the payment of each of its placements in order. The `request_id` makes a retry safe (AIP-155).
#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct BookSlot {
    #[schema(value_type = String, format = Uuid)]
    org_id: Uuid,
    slot: SlotBody,
    payments: Vec<Payment>,
    request_id: String,
}

/// The slot an appointment moves to.
#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct MoveAppointment {
    #[schema(value_type = String, format = Uuid)]
    org_id: Uuid,
    #[schema(value_type = String, format = Uuid)]
    id: Uuid,
    slot: SlotBody,
}

/// `id` is the id of the appointment and the method after a colon: `<id>:book` or `<id>:confirm`.
#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct AppointmentMethod {
    #[schema(value_type = String, format = Uuid)]
    org_id: Uuid,
    id: String,
    /// What pays for a held appointment that is booked.
    payment: Option<Payment>,
}

/// The orgs a request covers: the org of the path and those grouped under it.
async fn scope(pool: &PgPool, org: Uuid) -> Result<Vec<Uuid>, ApiError> {
    zone_of(pool, org).await?;
    Ok(subtree(pool, org).await?)
}

async fn found(pool: &PgPool, scope: &[Uuid], id: Uuid) -> ApiResult<AppointmentRow> {
    let query = "SELECT * FROM appointments WHERE id = $1 AND org_id = ANY($2) AND (status <> 'held' OR expires_at > now())";
    let row = sqlx::query_as(query).bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("appointment {id} not found")))
}

/// The payers and the methods of the payments must belong to the org or an org grouped under it.
async fn check_payments(pool: &PgPool, scope: &[Uuid], payments: &[Payment]) -> Result<(), ApiError> {
    let payers: Vec<Uuid> = payments.iter().map(|payment| payment.payer_id.0).collect();
    let methods: Vec<Uuid> = payments.iter().map(|payment| payment.method.0).collect();
    for (table, ids) in [("clients", payers), ("payment_methods", methods)] {
        let owners: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!("SELECT id, org_id FROM {table} WHERE id = ANY($1) AND deleted_at IS NULL")).bind(&ids).fetch_all(pool).await?;
        if let Some(id) = ids.iter().find(|id| !owners.iter().any(|(found, owner)| found == *id && scope.contains(owner))) {
            return Err(ApiError::not_found(format!("{table} {id} not found")));
        }
    }
    Ok(())
}

fn expiry(until: DateTime<Utc>) -> Result<DateTime<Utc>, ApiError> {
    let now = Utc::now();
    match (until > now, until - now <= Duration::hours(LONGEST_HOLD_HOURS)) {
        (false, _) => Err(ApiError::invalid("expires_at must be in the future")),
        (_, false) => Err(ApiError::invalid(format!("a hold lasts at most {LONGEST_HOLD_HOURS} hours"))),
        _ => Ok(until),
    }
}

fn all_of(appointments: Vec<AppointmentRow>) -> Paginated<AppointmentRow> {
    Paginated { data: appointments, next_page_token: String::new() }
}

/// List appointments, held ones that have not run out included.
#[utoipa::path(get, path = "/api/v1/orgs/{org_id}/appointments", tag = "appointments", params(("org_id" = Uuid, Path), ListQuery),
    responses((status = OK, body = Paginated<AppointmentRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListAppointments>) -> ApiResult<Paginated<AppointmentRow>> {
    let scope = scope(&ctx.pool, input.org_id).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let query = format!("SELECT * FROM appointments WHERE org_id = ANY($1){shown} AND (status <> 'held' OR expires_at > now()) ORDER BY starts_at, id");
    let rows = sqlx::query_as(&query).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

/// Keep a slot until `expires_at`: one `held` appointment for each placement.
#[utoipa::path(post, path = "/api/v1/orgs/{org_id}/appointments:hold", tag = "appointments", params(("org_id" = Uuid, Path)), request_body = HoldSlot,
    responses((status = OK, body = Paginated<AppointmentRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
pub(super) async fn hold(State(ctx): State<AppContext>, Params(input, _): Params<HoldSlot>) -> ApiResult<Paginated<AppointmentRow>> {
    let zone = zone_of(&ctx.pool, input.org_id).await?;
    let until = expiry(input.expires_at)?;
    let (request, slot) = verified(&ctx.pool, input.org_id, zone, &input.slot, None).await?;
    Ok(all_of(Reservations::new(ctx.pool.clone()).hold(&request, &slot, until).await?))
}

/// Book a slot: one appointment for each placement, `booked` while its rooms have capacity, else `waitlist`.
#[utoipa::path(post, path = "/api/v1/orgs/{org_id}/appointments", tag = "appointments", params(("org_id" = Uuid, Path)), request_body = BookSlot,
    responses((status = OK, body = Paginated<AppointmentRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
pub(super) async fn book(State(ctx): State<AppContext>, Params(input, _): Params<BookSlot>) -> ApiResult<Paginated<AppointmentRow>> {
    let zone = zone_of(&ctx.pool, input.org_id).await?;
    check_payments(&ctx.pool, &scope(&ctx.pool, input.org_id).await?, &input.payments).await?;
    let (reservations, key) = (Reservations::new(ctx.pool.clone()), IdempotencyKey(input.request_id));
    if let Some(earlier) = reservations.booked_under(&key, &booking_print(&claimed_placements(&input.slot, zone)?, &input.payments)).await? {
        return Ok(all_of(earlier));
    }
    let (request, slot) = verified(&ctx.pool, input.org_id, zone, &input.slot, None).await?;
    Ok(all_of(reservations.book(&request, &slot, input.payments, key).await?))
}

/// Get an appointment.
#[utoipa::path(get, path = "/api/v1/orgs/{org_id}/appointments/{id}", tag = "appointments", params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = AppointmentRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<AppointmentRef>) -> ApiResult<AppointmentRow> {
    found(&ctx.pool, &scope(&ctx.pool, input.org_id).await?, input.id).await
}

/// Move an appointment to a slot of the same client and service.
#[utoipa::path(patch, path = "/api/v1/orgs/{org_id}/appointments/{id}", tag = "appointments", params(("org_id" = Uuid, Path), ("id" = Uuid, Path)), request_body = MoveAppointment,
    responses((status = OK, body = AppointmentRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
pub(super) async fn reschedule(State(ctx): State<AppContext>, Params(input, _): Params<MoveAppointment>) -> ApiResult<AppointmentRow> {
    let zone = zone_of(&ctx.pool, input.org_id).await?;
    found(&ctx.pool, &subtree(&ctx.pool, input.org_id).await?, input.id).await?;
    let (request, slot) = verified(&ctx.pool, input.org_id, zone, &input.slot, Some(input.id)).await?;
    Ok(Reservations::new(ctx.pool.clone()).reschedule(AppointmentId(input.id), &request, &slot).await?)
}

/// Cancel an appointment. It stays, with the status `cancelled`.
#[utoipa::path(delete, path = "/api/v1/orgs/{org_id}/appointments/{id}", tag = "appointments", params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = AppointmentRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn cancel(State(ctx): State<AppContext>, Params(input, _): Params<AppointmentRef>) -> ApiResult<AppointmentRow> {
    let row = found(&ctx.pool, &scope(&ctx.pool, input.org_id).await?, input.id).await?;
    if row.deleted_at.is_some() {
        return Err(ApiError::not_found(format!("appointment {} is already cancelled", input.id)));
    }
    Ok(Reservations::new(ctx.pool.clone()).transition(row.id, Status::Cancelled).await?)
}

/// `:book` books a held appointment with `payment`, or a waitlisted one if there is capacity; `:confirm` confirms a
/// booked one. The id in the path is followed by the method: `<id>:book`, `<id>:confirm`.
#[utoipa::path(post, path = "/api/v1/orgs/{org_id}/appointments/{id}", tag = "appointments", params(("org_id" = Uuid, Path), ("id" = String, Path, description = "The id and the method: `<id>:book` or `<id>:confirm`")), request_body = AppointmentMethod,
    responses((status = OK, body = AppointmentRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
pub(super) async fn method(State(ctx): State<AppContext>, Params(input, _): Params<AppointmentMethod>) -> ApiResult<AppointmentRow> {
    let (id, verb) = input.id.split_once(':').ok_or_else(|| ApiError::not_found(format!("{:?} is not a method of appointments", input.id)))?;
    let id: Uuid = id.parse().map_err(|_| ApiError::invalid(format!("{id:?} is not an appointment id")))?;
    let scope = scope(&ctx.pool, input.org_id).await?;
    let row = found(&ctx.pool, &scope, id).await?;
    let reservations = Reservations::new(ctx.pool.clone());
    match (verb, row.status) {
        ("confirm", _) => Ok(reservations.transition(row.id, Status::Confirmed).await?),
        ("book", Status::Held) => {
            let payment = input.payment.ok_or_else(|| ApiError::invalid("a held appointment is booked with a payment"))?;
            check_payments(&ctx.pool, &scope, std::slice::from_ref(&payment)).await?;
            Ok(reservations.book_held(row.id, payment, IdempotencyKey(format!("{id}:book"))).await?)
        }
        ("book", _) => Ok(reservations.transition(row.id, Status::Booked).await?),
        _ => Err(ApiError::not_found(format!("{verb:?} is not a method of appointments"))),
    }
}
