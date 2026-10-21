//! Appointment: one client, one service, one room and zero or more providers for the client's time.
//!
//! Its status moves only through `:book`, `:confirm`, `:complete` (AIP-136) and DELETE, which cancels.
//! Appointments are created by booking (availability search and reservations), not by a create route.

use axum::extract::State;
use axum::routing::post;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use sqlx::types::Json;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::value::{Attributes, Name};
use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};
use crate::{Entity, Table};

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
        .routes(routes!(list))
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
    change_status(&ctx.pool, &key, CANCEL, None).await
}

/// `:book` a waitlisted appointment; `:confirm` a booked one; `:complete` a booked or confirmed one.
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
    responses((status = 200, body = Entity<Uuid, AppointmentData>), (status = 400), (status = 404)),
)]
async fn act(State(ctx): State<AppContext>, Input(action): Input<Action>) -> ApiResult<Appointment> {
    change_status(&ctx.pool, &action.key, action.verb.transition(), action.payment.as_ref()).await
}

/// One guarded UPDATE, so of two requests at once only one can move the status.
async fn change_status(
    pool: &PgPool,
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
    .fetch_optional(pool)
    .await?;
    match changed {
        Some(appointment) => Ok(appointment),
        None => Err(refusal(pool, key, to).await),
    }
}

/// Why a status change matched no row: the appointment is missing, or its status cannot move to `to`.
async fn refusal(pool: &PgPool, key: &Key, to: Status) -> ApiError {
    match crud::get::<AppointmentData>(pool, key).await {
        Ok(current) => ApiError::BadRequest(format!("a {:?} appointment cannot become {to:?}", current.data.status)),
        Err(error) => error,
    }
}
