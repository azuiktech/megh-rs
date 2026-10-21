//! Appointment: one client, one service, one room and zero or more providers for the client's time.
//!
//! Booking creates it only when its time is open (the check of the slot search). Its status then moves only
//! through `:book`, `:confirm`, `:complete` (AIP-136) and DELETE, which cancels.

use axum::extract::State;
use axum::routing::post;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use itertools::Itertools;
use sqlx::{PgConnection, PgPool};
use sqlx::types::Json;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};
use crate::{Entity, Table, TableEntity};

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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "appointments", keys = ["org_id", "id"])]
#[validate(schema(function = "ends_after_start"))]
pub struct AppointmentData {
    pub org_id: Uuid,
    pub client_id: Uuid,
    pub service_id: Uuid,
    pub resource_id: Uuid,
    #[serde(default)]
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
    /// Set by the server when the appointment is stored.
    #[serde(default)]
    #[schema(read_only)]
    pub booked_at: DateTime<Utc>,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Appointment = Entity<Uuid, AppointmentData>;

fn ends_after_start(appointment: &AppointmentData) -> Result<(), ValidationError> {
    (appointment.starts_at < appointment.ends_at)
        .then_some(())
        .ok_or_else(|| ValidationError::new("ends_at must be after starts_at"))
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

/// Fields a PATCH may change until availability search can check a move.
const CHANGEABLE: [&str; 5] = ["org_id", "id", "note", "payment", "attributes"];
/// Fields only the server sets; a PATCH that sends them keeps the stored values.
const SERVER_SET: [&str; 2] = ["status", "booked_at"];

#[derive(OpenApi)]
#[openapi(paths(act))]
struct Actions;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Actions::openapi())
        .routes(routes!(list, create))
        .routes(routes!(get, patch, delete))
        .route("/orgs/{org_id}/appointments/{id}/{verb}", post(act))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/appointments",
    tag = "appointments",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter"),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = Page<Entity<Uuid, AppointmentData>>)),
)]
async fn list(
    State(ctx): State<AppContext>,
    Input(request): Input<ListRequest>,
) -> ApiResult<Page<Appointment>> {
    crud::list(&ctx.pool, &request).await
}

/// Book: stored only if its time is open, with a free place for `booked` or a waitlist place for `waitlist`.
/// Checked and written under one lock per org, so two bookings cannot take the same place.
#[utoipa::path(
    post,
    path = "/orgs/{org_id}/appointments",
    tag = "appointments",
    params(("org_id" = Uuid, Path)),
    request_body = AppointmentData,
    responses((status = 200, body = Entity<Uuid, AppointmentData>), (status = 400), (status = 409)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<AppointmentData>) -> ApiResult<Appointment> {
    [Status::Booked, Status::Waitlist]
        .contains(&data.status)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("a new appointment is booked or waitlist".to_string()))?;
    let mut transaction = ctx.pool.begin().await?;
    lock(&mut transaction, data.org_id).await?;
    let taken = super::availability::fit(&mut transaction, &data, None).await?;
    let appointment = Entity::new(Uuid::now_v7(), AppointmentData { booked_at: Utc::now(), ..data });
    let saved = TableEntity::insert(&appointment, &mut *transaction).await?;
    occupy(&mut transaction, &saved, &taken).await?;
    transaction.commit().await?;
    Ok(saved)
}

/// Bookings of one org wait for each other until their transaction ends, so two cannot take one place.
async fn lock(conn: &mut PgConnection, org_id: Uuid) -> ApiResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .bind(org_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// Stores the time the appointment takes from each participant.
async fn occupy(conn: &mut PgConnection, appointment: &Appointment, taken: &[(Uuid, (DateTime<Utc>, DateTime<Utc>))]) -> ApiResult<()> {
    let (participants, starts, ends): (Vec<_>, Vec<_>, Vec<_>) = taken.iter().map(|(id, (start, end))| (*id, *start, *end)).multiunzip();
    sqlx::query(
        "INSERT INTO occupancies (org_id, appointment_id, participant_id, during) \
         SELECT $1, $2, participant, tstzrange(starts, ends) FROM unnest($3::uuid[], $4::timestamptz[], $5::timestamptz[]) AS t(participant, starts, ends)",
    )
    .bind(appointment.data.org_id)
    .bind(appointment.id)
    .bind(participants)
    .bind(starts)
    .bind(ends)
    .execute(conn)
    .await?;
    Ok(())
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/appointments/{id}",
    tag = "appointments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, AppointmentData>), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Appointment> {
    crud::get(&ctx.pool, &key).await
}

/// Change the note, payment or attributes (JSON merge patch, RFC 7396). `status` and `booked_at` are ignored.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/appointments/{id}",
    tag = "appointments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = Entity<Uuid, AppointmentData>), (status = 400), (status = 404)),
)]
async fn patch(
    State(ctx): State<AppContext>,
    Input(mut body): Input<Changes>,
) -> ApiResult<Appointment> {
    body.changes.retain(|field, _| !SERVER_SET.contains(&field.as_str()));
    match body.changes.keys().find(|field| !CHANGEABLE.contains(&field.as_str())) {
        Some(field) => Err(ApiError::BadRequest(format!("{field} cannot be changed"))),
        None => crud::patch(&ctx.pool, body).await,
    }
}

/// Cancel: a waitlisted, booked or confirmed appointment becomes cancelled.
#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/appointments/{id}",
    tag = "appointments",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Entity<Uuid, AppointmentData>), (status = 400), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Appointment> {
    change_status(&mut *ctx.pool.acquire().await?, &key, CANCEL, None).await
}

/// `:book` a waitlisted appointment when a free place is left for its time; `:confirm` a booked one;
/// `:complete` a booked or confirmed one.
#[utoipa::path(
    post,
    path = "/orgs/{org_id}/appointments/{id}:{verb}",
    tag = "appointments",
    params(
        ("org_id" = Uuid, Path),
        ("id" = Uuid, Path),
        ("verb" = String, Path, description = "book, confirm or complete"),
    ),
    request_body(content = Object, description = "`{\"payment\": Payment}` to replace the payment, or `{}`"),
    responses((status = 200, body = Entity<Uuid, AppointmentData>), (status = 400), (status = 404), (status = 409)),
)]
async fn act(State(ctx): State<AppContext>, Input(action): Input<Action>) -> ApiResult<Appointment> {
    match action.verb {
        Verb::Book => book(&ctx.pool, &action).await,
        _ => change_status(&mut *ctx.pool.acquire().await?, &action.key, action.verb.transition(), action.payment.as_ref()).await,
    }
}

/// Waitlist to booked, under the booking lock, only when the booking check finds a free place for the
/// appointment's time, leaving out the appointment's own time.
async fn book(pool: &PgPool, action: &Action) -> ApiResult<Appointment> {
    let mut transaction = pool.begin().await?;
    lock(&mut transaction, action.key.org_id).await?;
    let waiting: Option<Appointment> = sqlx::query_as(
        "SELECT * FROM appointments WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL AND status = 'waitlist' FOR UPDATE",
    )
    .bind(action.key.org_id)
    .bind(action.key.id)
    .fetch_optional(&mut *transaction)
    .await?;
    if let Some(waiting) = waiting {
        let booked = AppointmentData { status: Status::Booked, ..waiting.data };
        super::availability::fit(&mut transaction, &booked, Some(waiting.id)).await?;
    }
    let booked = change_status(&mut transaction, &action.key, action.verb.transition(), action.payment.as_ref()).await?;
    transaction.commit().await?;
    Ok(booked)
}

/// One guarded UPDATE, so of two requests at once only one can move the status.
async fn change_status(
    conn: &mut PgConnection,
    key: &Key,
    (to, from): Transition,
    payment: Option<&Payment>,
) -> ApiResult<Appointment> {
    let changed = sqlx::query_as(
        "UPDATE appointments SET status = $3, payment = COALESCE($4, payment), updated_at = now() \
         WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL AND status = ANY($5) RETURNING *",
    )
    .bind(key.org_id)
    .bind(key.id)
    .bind(to)
    .bind(payment.map(Json))
    .bind(from)
    .fetch_optional(&mut *conn)
    .await?;
    match changed {
        Some(appointment) => Ok(appointment),
        None => Err(refusal(conn, key, to).await),
    }
}

/// Why a status change matched no row: the appointment is missing, or its status cannot move to `to`.
async fn refusal(conn: &mut PgConnection, key: &Key, to: Status) -> ApiError {
    let current: Result<Option<Status>, sqlx::Error> =
        sqlx::query_scalar("SELECT status FROM appointments WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL")
            .bind(key.org_id)
            .bind(key.id)
            .fetch_optional(conn)
            .await;
    match current {
        Ok(Some(status)) => ApiError::BadRequest(format!("a {status:?} appointment cannot become {to:?}")),
        Ok(None) => ApiError::NotFound,
        Err(error) => error.into(),
    }
}
