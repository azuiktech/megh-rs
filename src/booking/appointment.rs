//! Appointment: one client, one service, one room and zero or more providers for the client's time.
//!
//! Booking creates it only when its time is open (the check of the slot search). Its status then moves only
//! through `:book`, `:confirm`, `:complete` (AIP-136) and DELETE, which cancels.

use axum::extract::State;
use axum::routing::post;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use crate::{Db, Tx};
use sqlx::types::Json;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::Array;
use validator::{Validate, ValidationError};

use super::availability::fit;
use super::reservation::{claim, give_back, lock, take};
use super::stock::{ensure_on_hand, Used};
use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};

/// Who pays and how: card, cash, a pass, a voucher, insurance; the details go in `attributes`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, ToSchema)]
pub struct Payment {
    /// A client; not always the appointment's client (a parent paying for a child).
    pub payer_id: Uuid,
    #[schema(value_type = String, example = "group insurance")]
    pub method: Name,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Waitlist,
    Booked,
    Confirmed,
    Completed,
    Cancelled,
}

#[crate::entity(table = "appointments", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema)]
#[validate(schema(function = "ends_after_start"))]
pub struct Appointment {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
    pub client_id: Uuid,
    pub service_id: Uuid,
    pub resource_id: Uuid,
    #[serde(default)]
    #[sqlx(try_from = "Array<Uuid>")]
    pub provider_ids: Vec<Uuid>,
    /// The client's time; the service's setup and cleanup are outside it.
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: Status,
    #[serde(default)]
    pub note: String,
    #[sqlx(json)]
    #[validate(nested)]
    pub payment: Payment,
    /// The products it uses; they must be on hand when it is booked or they change.
    #[serde(default)]
    #[sqlx(json)]
    #[validate(nested)]
    pub products: Vec<Used>,
    /// Set by the server when the appointment is stored.
    #[serde(default)]
    #[schema(read_only)]
    pub booked_at: DateTime<Utc>,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

fn ends_after_start(appointment: &Appointment) -> Result<(), ValidationError> {
    (appointment.starts_at < appointment.ends_at)
        .then_some(())
        .ok_or_else(|| ValidationError::new("ends_at must be after starts_at"))
}

/// A booking: the appointment, and the hold it takes its places from, if any.
#[derive(Debug, Deserialize, Validate, ToSchema)]
#[schema(example = json!({"client_id": "01a15d4f-446c-711b-8ef2-4634de693900", "service_id": "01a15d4f-448f-755a-85d6-3f42464b40df", "resource_id": "01a15d4f-4455-7343-a146-5f04bee09dca", "provider_ids": [], "starts_at": "2026-10-26T09:00:00+05:30", "ends_at": "2026-10-26T10:00:00+05:30", "status": "booked", "payment": {"payer_id": "01a15d4f-446c-711b-8ef2-4634de693900", "method": "card"}, "reservation_id": null}))]
pub struct Booking {
    #[serde(flatten)]
    #[validate(nested)]
    pub appointment: Appointment,
    pub reservation_id: Option<Uuid>,
}

/// The custom methods; the path segment is `:book`, `:confirm` or `:complete`.
#[derive(Debug, Clone, Copy, Deserialize)]
pub enum Verb {
    #[serde(rename = ":book")]
    Book,
    #[serde(rename = ":confirm")]
    Confirm,
    #[serde(rename = ":complete")]
    Complete,
}

/// A status and the statuses it may be reached from.
type Transition = (Status, &'static [Status]);

const CANCEL: Transition = (Status::Cancelled, &[Status::Waitlist, Status::Booked, Status::Confirmed]);

impl Verb {
    fn transition(self) -> Transition {
        match self {
            Self::Book => (Status::Booked, &[Status::Waitlist]),
            Self::Confirm => (Status::Confirmed, &[Status::Booked]),
            Self::Complete => (Status::Completed, &[Status::Booked, Status::Confirmed]),
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
pub struct Action {
    #[serde(flatten)]
    pub key: Key,
    pub verb: Verb,
    /// Replaces the payment when given.
    #[validate(nested)]
    pub payment: Option<Payment>,
}

/// Fields a PATCH may change without moving the appointment.
const CHANGEABLE: [&str; 6] = ["tenant_id", "id", "note", "payment", "products", "attributes"];
/// Fields that move the appointment; a move is checked like a booking.
const MOVES: [&str; 4] = ["starts_at", "ends_at", "resource_id", "provider_ids"];
/// Fields only the server sets; a PATCH that sends them keeps the stored values.
const SERVER_SET: [&str; 2] = ["status", "booked_at"];

#[derive(OpenApi)]
#[openapi(paths(act))]
struct Actions;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Actions::openapi())
        .routes(routes!(list, create))
        .routes(routes!(get, patch, delete))
        .route("/tenants/{tenant_id}/appointments/{id}/{verb}", post(act))
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/appointments",
    tag = "appointments",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`. Example: `role = \"hygienist\" AND capacity >= 2`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Appointment>))),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Appointment>> {
    crud::list(&ctx.pool, &request).await
}

/// Book: stored only if its time is open, with a free place for `booked` or a waitlist place for `waitlist`;
/// with `reservation_id`, from that hold's places. Checked and written under one lock per tenant, so two bookings
/// cannot take the same place.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/appointments",
    tag = "appointments",
    params(("tenant_id" = Uuid, Path)),
    request_body = Booking,
    responses((status = 200, body = inline(Appointment)), (status = 400), (status = 409)),
)]
async fn create(State(ctx): State<AppContext>, Input(booking): Input<Booking>) -> ApiResult<Appointment> {
    let data = booking.appointment;
    [Status::Booked, Status::Waitlist]
        .contains(&data.status)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("a new appointment is booked or waitlist".to_string()))?;
    let mut transaction = ctx.pool.begin().await?;
    lock(&mut transaction, data.tenant_id).await?;
    ensure_on_hand(&mut transaction, data.tenant_id, &data.products).await?;
    let places = fit(&mut transaction, &data, booking.reservation_id).await?;
    if let Some(hold) = booking.reservation_id {
        claim(&mut transaction, data.tenant_id, hold, &places).await?;
    }
    let saved = crud::create(&mut transaction, Appointment { booked_at: Utc::now(), ..data }, None).await?;
    take(&mut transaction, saved.tenant_id, saved.id, &places, None).await?;
    transaction.commit().await?;
    Ok(saved)
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/appointments/{id}",
    tag = "appointments",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Appointment)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Appointment> {
    crud::get(&ctx.pool, &key).await
}

/// Change the note, payment or attributes, or move the appointment to another time, room or providers (JSON
/// merge patch, RFC 7396). A move is checked like a booking. `status` and `booked_at` are ignored.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/appointments/{id}",
    tag = "appointments",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = Appointment, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Appointment)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(
    State(ctx): State<AppContext>,
    Input(mut body): Input<Changes>,
) -> ApiResult<Appointment> {
    body.changes.retain(|field, _| !SERVER_SET.contains(&field.as_str()));
    let is = |fields: &[&str], field: &String| fields.contains(&field.as_str());
    if let Some(field) = body.changes.keys().find(|field| !is(&CHANGEABLE, field) && !is(&MOVES, field)) {
        return Err(ApiError::BadRequest(format!("{field} cannot be changed")));
    }
    ensure_products(&ctx.pool, &body).await?;
    match body.changes.keys().any(|field| is(&MOVES, field)) {
        true => move_to(&ctx.pool, body).await,
        false => crud::patch(&ctx.pool, body, None).await,
    }
}

/// Products a PATCH sets must be on hand.
async fn ensure_products(pool: &Db, body: &Changes) -> ApiResult<()> {
    let used: Vec<Used> = body
        .changes
        .get("products")
        .filter(|products| !products.is_null())
        .map(|products| serde_json::from_value(products.clone()))
        .transpose()
        .map_err(|error| ApiError::BadRequest(error.to_string()))?
        .unwrap_or_default();
    ensure_on_hand(&mut pool.begin().await?, body.key.tenant_id, &used).await
}

/// A move: under the booking lock, the changed appointment must pass the booking check for its status,
/// leaving out its own time; its reservations are written again.
async fn move_to(pool: &Db, body: Changes) -> ApiResult<Appointment> {
    let (tenant_id, id) = (body.key.tenant_id, body.key.id);
    let mut transaction = pool.begin().await?;
    lock(&mut transaction, tenant_id).await?;
    let moved: Appointment = crud::patch(&mut transaction, body, None).await?;
    [Status::Waitlist, Status::Booked, Status::Confirmed]
        .contains(&moved.status)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest(format!("a {:?} appointment cannot move", moved.status)))?;
    let places = fit(&mut transaction, &moved, Some(moved.id)).await?;
    give_back(&mut transaction, tenant_id, id).await?;
    take(&mut transaction, tenant_id, id, &places, None).await?;
    transaction.commit().await?;
    Ok(moved)
}

/// Cancel: a waitlisted, booked or confirmed appointment becomes cancelled.
#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/appointments/{id}",
    tag = "appointments",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Appointment)), (status = 400), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Appointment> {
    let mut transaction = ctx.pool.begin().await?;
    let cancelled = change_status(&mut transaction, &key, CANCEL, None).await?;
    give_back(&mut transaction, key.tenant_id, key.id).await?;
    transaction.commit().await?;
    Ok(cancelled)
}

/// `:book` a waitlisted appointment when a free place is left for its time; `:confirm` a booked one;
/// `:complete` a booked or confirmed one.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/appointments/{id}:{verb}",
    tag = "appointments",
    params(
        ("tenant_id" = Uuid, Path),
        ("id" = Uuid, Path),
        ("verb" = String, Path, description = "book, confirm or complete"),
    ),
    request_body(content = Object, description = "`{\"payment\": Payment}` to replace the payment, or `{}`"),
    responses((status = 200, body = inline(Appointment)), (status = 400), (status = 404), (status = 409)),
)]
async fn act(State(ctx): State<AppContext>, Input(action): Input<Action>) -> ApiResult<Appointment> {
    match action.verb {
        Verb::Book => book(&ctx.pool, &action).await,
        _ => {
            let mut transaction = ctx.pool.begin().await?;
            let changed = change_status(&mut transaction, &action.key, action.verb.transition(), action.payment.as_ref()).await?;
            transaction.commit().await?;
            Ok(changed)
        }
    }
}

/// Waitlist to booked, under the booking lock, only when the booking check finds a free place for the
/// appointment's time, leaving out the appointment's own time.
async fn book(pool: &Db, action: &Action) -> ApiResult<Appointment> {
    let mut transaction = pool.begin().await?;
    lock(&mut transaction, action.key.tenant_id).await?;
    let waiting: Option<Appointment> = Db::query_as(
        "SELECT * FROM appointments WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL AND status = 'waitlist' FOR UPDATE",
    )
    .sqlite("SELECT * FROM appointments WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL AND status = 'waitlist'")
    .bind(action.key.tenant_id)
    .bind(action.key.id)
    .fetch_optional(&mut transaction)
    .await?;
    if let Some(waiting) = waiting {
        let booked = Appointment { status: Status::Booked, ..waiting };
        fit(&mut transaction, &booked, Some(waiting.id)).await?;
    }
    let booked = change_status(&mut transaction, &action.key, action.verb.transition(), action.payment.as_ref()).await?;
    Db::query("UPDATE reservations SET waitlist = false WHERE tenant_id = $1 AND owner_id = $2")
        .bind(action.key.tenant_id)
        .bind(action.key.id)
        .execute(&mut transaction)
        .await?;
    transaction.commit().await?;
    Ok(booked)
}

/// One guarded UPDATE, so of two requests at once only one can move the status.
async fn change_status(
    conn: &mut Tx,
    key: &Key,
    (to, from): Transition,
    payment: Option<&Payment>,
) -> ApiResult<Appointment> {
    let changed = Db::query_as(
        "UPDATE appointments SET status = $3, payment = COALESCE($4, payment), updated_at = now() \
         WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL AND status = ANY($5) RETURNING *",
    )
    .sqlite(concat!(
        "UPDATE appointments SET status = $3, payment = COALESCE($4, payment), updated_at = ", crate::sqlite_now!(), " \
         WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL AND status IN (SELECT value FROM json_each($5)) RETURNING *",
    ))
    .bind(key.tenant_id)
    .bind(key.id)
    .bind(to)
    .bind(payment.map(Json))
    .bind(Array(from.to_vec()))
    .fetch_optional(&mut *conn)
    .await?;
    match changed {
        Some(appointment) => Ok(appointment),
        None => Err(refusal(conn, key, to).await),
    }
}

/// Why a status change matched no row: the appointment is missing, or its status cannot move to `to`.
async fn refusal(conn: &mut Tx, key: &Key, to: Status) -> ApiError {
    let current: Result<Option<Status>, sqlx::Error> =
        Db::query_scalar("SELECT status FROM appointments WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL")
            .bind(key.tenant_id)
            .bind(key.id)
            .fetch_optional(conn)
            .await;
    match current {
        Ok(Some(status)) => ApiError::BadRequest(format!("a {status:?} appointment cannot become {to:?}")),
        Ok(None) => ApiError::NotFound,
        Err(error) => error.into(),
    }
}
