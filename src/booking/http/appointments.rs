//! `/api/v1/orgs/{org_id}/appointments`: held and booked appointments, as one resource with a `state`.
//!
//! - `POST .../appointments` with `state` `held` (blocks the slot until `expire_time`) or `booked` (needs a `request_id`,
//!   AIP-155, so a retry returns the same appointment);
//! - `POST .../appointments/{id}:confirm` (one route, `{id}`, where the handler reads the `:verb`: axum does not match a
//!   suffix after a parameter) turns a held appointment into a booked one, under the same id;
//! - `GET` one or a list (`filter`, `page_size`, `page_token`, `show_deleted`), and `DELETE` to cancel (AIP-164).

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use axum_extra::extract::{Query, WithRejection};
use chrono::{Duration, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::common::{instant, zone_of, AmountBody};
use super::error::ApiError;
use super::page::{page, ListQuery};
use super::slot::{verified, SlotBody};
use super::view;
use super::{subtree, BookingState};
use crate::booking::{
    AppointmentId, AppointmentRow, Appointments, Attributes, ClientId, Confirmation, Error, HoldId, HoldRow, IdempotencyKey, PaymentMethodId, Reservations,
};
use crate::TableEntity;

const DEFAULT_HOLD_MINUTES: i64 = 10;
const LONGEST_HOLD_HOURS: i64 = 24;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum StateBody {
    Held,
    Booked,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PaymentBody {
    method: PaymentMethodId,
    amount: AmountBody,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateBody {
    state: StateBody,
    slot: SlotBody,
    expire_time: Option<String>,
    request_id: Option<String>,
    payer_id: Option<ClientId>,
    clients: Option<Vec<ClientId>>,
    payments: Option<Vec<PaymentBody>>,
    attributes: Option<Attributes>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfirmBody {
    request_id: String,
    payer_id: ClientId,
    clients: Option<Vec<ClientId>>,
    payments: Option<Vec<PaymentBody>>,
    attributes: Option<Attributes>,
}

pub(super) fn routes() -> Router<BookingState> {
    let collection = "/api/v1/orgs/{org_id}/appointments";
    Router::new()
        .route(collection, get(list).post(create))
        .route(&format!("{collection}/{{id}}"), get(read).delete(cancel).post(custom_method))
}

fn confirmation(clients: Option<Vec<ClientId>>, payer: ClientId, payments: Option<Vec<PaymentBody>>, attributes: Option<Attributes>) -> Result<Confirmation, ApiError> {
    let payments = payments.iter().flatten().map(|p| Ok(crate::booking::Payment { method: p.method, amount: p.amount.amount()? })).collect::<Result<_, ApiError>>()?;
    Ok(Confirmation { clients: clients.unwrap_or_default(), payer, payments, attributes: attributes.unwrap_or_default() })
}

/// The payer, the clients and the payment methods must belong to the org's subtree or to no org.
async fn check_parties(pool: &PgPool, org: Uuid, confirmation: &Confirmation) -> Result<(), ApiError> {
    let scope = subtree(pool, org).await?;
    let clients: Vec<Uuid> = confirmation.clients.iter().chain([&confirmation.payer]).map(|client| client.0).collect();
    let methods: Vec<Uuid> = confirmation.payments.iter().map(|payment| payment.method.0).collect();
    for (table, ids) in [("clients", clients), ("payment_methods", methods)] {
        let owners: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(&format!("SELECT id, org_id FROM {table} WHERE id = ANY($1) AND deleted_at IS NULL")).bind(&ids).fetch_all(pool).await?;
        let missing = ids.iter().find(|id| !owners.iter().any(|(found, owner)| found == *id && owner.is_none_or(|owner| scope.contains(&owner))));
        if let Some(id) = missing {
            return Err(ApiError::not_found(format!("{table} {id} not found")));
        }
    }
    Ok(())
}

/// A hold that is still held, or that was released (deleted); one that ran out is gone.
async fn held_in(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<HoldRow, ApiError> {
    let query = "SELECT * FROM holds WHERE id = $1 AND org_id = ANY($2) AND (deleted_at IS NOT NULL OR expires_at > now())";
    let row = sqlx::query_as(query).bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("appointment {id} not found")))
}

async fn live_hold(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<HoldRow, ApiError> {
    let hold = held_in(pool, scope, id).await?;
    match hold.deleted_at {
        None => Ok(hold),
        Some(_) => Err(ApiError::not_found(format!("appointment {id} is already cancelled"))),
    }
}

async fn appointment_in(pool: &PgPool, scope: &[Uuid], id: Uuid) -> Result<Option<AppointmentRow>, ApiError> {
    Ok(sqlx::query_as("SELECT * FROM appointments WHERE id = $1 AND org_id = ANY($2)").bind(id).bind(scope).fetch_optional(pool).await?)
}

async fn resource_of(pool: &PgPool, org: Uuid, scope: &[Uuid], id: Uuid) -> Result<Value, ApiError> {
    let held = view::occupancies(pool, &[id]).await?;
    let rows = held.get(&id).map(|rows| rows.iter().collect::<Vec<_>>()).unwrap_or_default();
    match appointment_in(pool, scope, id).await? {
        Some(row) => Ok(view::appointment(&row, &rows, org)),
        None => Ok(view::hold(&held_in(pool, scope, id).await?, &rows, org)),
    }
}

fn expiry(body: &CreateBody, zone: Tz) -> Result<chrono::DateTime<Utc>, ApiError> {
    let now = Utc::now();
    let until = body.expire_time.as_deref().map(|text| instant(text, zone)).transpose()?.unwrap_or(now + Duration::minutes(DEFAULT_HOLD_MINUTES));
    match (until > now, until - now <= Duration::hours(LONGEST_HOLD_HOURS)) {
        (false, _) => Err(ApiError::invalid("expire_time must be in the future")),
        (_, false) => Err(ApiError::invalid(format!("a hold lasts at most {LONGEST_HOLD_HOURS} hours"))),
        _ => Ok(until),
    }
}

async fn create(
    State(state): State<BookingState>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<CreateBody>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let (pool, zone) = (&state.pool, zone_of(&state.pool, org).await?);
    let scope = subtree(pool, org).await?;
    match body.state {
        StateBody::Held => hold_only(pool, org, zone, &scope, body).await,
        StateBody::Booked => book(pool, org, zone, &scope, body).await,
    }
}

async fn hold_only(pool: &PgPool, org: Uuid, zone: Tz, scope: &[Uuid], body: CreateBody) -> Result<Json<Value>, ApiError> {
    if body.payer_id.is_some() || body.payments.is_some() || body.clients.is_some() || body.request_id.is_some() {
        return Err(ApiError::invalid("a held appointment has no payer, clients, payments or request_id yet; give them when confirming"));
    }
    let until = expiry(&body, zone)?;
    let (request, slot) = verified(pool, org, zone, &body.slot).await?;
    let hold = Reservations::new(pool.clone()).hold(&request, &slot, until).await?;
    Ok(Json(resource_of(pool, org, scope, hold.id.0).await?))
}

async fn book(pool: &PgPool, org: Uuid, zone: Tz, scope: &[Uuid], body: CreateBody) -> Result<Json<Value>, ApiError> {
    let (Some(request_id), Some(payer)) = (body.request_id.clone(), body.payer_id) else { return Err(ApiError::invalid("a booked appointment needs a request_id and a payer_id")) };
    if body.expire_time.is_some() {
        return Err(ApiError::invalid("a booked appointment has no expire_time"));
    }
    let key = IdempotencyKey(request_id);
    if let Some(earlier) = sqlx::query_as::<_, AppointmentRow>("SELECT * FROM appointments WHERE idempotency_key = $1 AND org_id = ANY($2)").bind(&key).bind(scope).fetch_optional(pool).await? {
        return Ok(Json(resource_of(pool, org, scope, earlier.id.0).await?));
    }
    let confirmed = confirmation(body.clients, payer, body.payments, body.attributes)?;
    check_parties(pool, org, &confirmed).await?;
    let (request, slot) = verified(pool, org, zone, &body.slot).await?;
    let reservations = Reservations::new(pool.clone());
    let hold = reservations.hold(&request, &slot, Utc::now() + Duration::minutes(1)).await?;
    match reservations.confirm(hold.id, confirmed, key).await {
        Ok(appointment) => Ok(Json(resource_of(pool, org, scope, appointment.id.0).await?)),
        Err(error) => {
            hold.soft_delete(pool).await?;
            Err(error.into())
        }
    }
}

fn plain_id(text: &str) -> Result<Uuid, ApiError> {
    text.parse().map_err(|_| ApiError::invalid(format!("{text:?} is not an appointment id")))
}

/// `POST .../appointments/{id}:verb`, a custom method (AIP-136).
async fn custom_method(
    State(state): State<BookingState>,
    WithRejection(Path((org, target)), _): WithRejection<Path<(Uuid, String)>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<Value>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    match target.split_once(':') {
        Some((id, "confirm")) => {
            let body: ConfirmBody = serde_json::from_value(body).map_err(|error| ApiError::invalid(error.to_string()))?;
            confirm(&state, org, plain_id(id)?, body).await
        }
        _ => Err(ApiError::not_found(format!("{target:?} is not a method of appointments"))),
    }
}

async fn confirm(state: &BookingState, org: Uuid, id: Uuid, body: ConfirmBody) -> Result<Json<Value>, ApiError> {
    let pool = &state.pool;
    zone_of(pool, org).await?;
    let scope = subtree(pool, org).await?;
    let key = IdempotencyKey(body.request_id);
    let confirmed = confirmation(body.clients, body.payer_id, body.payments, body.attributes)?;
    check_parties(pool, org, &confirmed).await?;
    if appointment_in(pool, &scope, id).await?.is_none() {
        live_hold(pool, &scope, id).await?;
    }
    let appointment = Reservations::new(pool.clone()).confirm(HoldId(id), confirmed, key).await.map_err(|error| match error {
        Error::NotFound => ApiError::not_found(format!("appointment {id} not found")),
        other => other.into(),
    })?;
    Ok(Json(resource_of(pool, org, &scope, appointment.id.0).await?))
}

async fn read(State(state): State<BookingState>, WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, String)>, ApiError>) -> Result<Json<Value>, ApiError> {
    zone_of(&state.pool, org).await?;
    let scope = subtree(&state.pool, org).await?;
    Ok(Json(resource_of(&state.pool, org, &scope, plain_id(&id)?).await?))
}

async fn list(
    State(state): State<BookingState>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Query(query), _): WithRejection<Query<ListQuery>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    zone_of(&state.pool, org).await?;
    let scope = subtree(&state.pool, org).await?;
    let items = view::all(&state.pool, &scope, org, query.show_deleted == Some(true)).await?;
    Ok(Json(page("appointments", items, &query)?))
}

async fn cancel(State(state): State<BookingState>, WithRejection(Path((org, id)), _): WithRejection<Path<(Uuid, String)>, ApiError>) -> Result<Json<Value>, ApiError> {
    let (pool, id) = (&state.pool, plain_id(&id)?);
    zone_of(pool, org).await?;
    let scope = subtree(pool, org).await?;
    match appointment_in(pool, &scope, id).await? {
        Some(row) if row.deleted_at.is_none() => Reservations::new(pool.clone()).cancel(AppointmentId(id)).await?,
        Some(_) => return Err(ApiError::not_found(format!("appointment {id} is already cancelled"))),
        None => {
            live_hold(pool, &scope, id).await?.soft_delete(pool).await?;
        }
    }
    Ok(Json(resource_of(pool, org, &scope, id).await?))
}
