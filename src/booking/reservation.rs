//! Reservations: the places taken from rooms, providers and clients, one row per place. An appointment owns
//! its places until it is cancelled; a hold owns a slot's places for a few minutes, so that the slot is not
//! offered to anyone else while it is being booked. A hold is not an appointment: it has no client or payment.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, TimeDelta, Utc};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use crate::{Array, Db, Tx};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::availability::{Place, Placed, fit_slot};
use crate::auth::rest::{ApiError, ApiResult, AppContext, Input, Key};

/// How long a hold keeps its places.
const HOLD_FOR: TimeDelta = TimeDelta::minutes(10);

/// A slot to hold, as the search returned it.
#[derive(Debug, Deserialize, Validate, ToSchema)]
pub struct Reserve {
    pub tenant_id: Uuid,
    #[validate(length(min = 1))]
    pub parties: Vec<Placed>,
}

/// A hold: book with its id as `reservation_id` before it expires.
#[derive(Debug, Serialize, ToSchema)]
pub struct Hold {
    pub id: Uuid,
    pub expires_at: DateTime<Utc>,
}

/// Places given back.
#[derive(Debug, Serialize, ToSchema)]
pub struct Released {
    pub places: u64,
}

/// Deletes the places of holds that expired and of times that ended before `before`, which is not in the future.
#[derive(Debug, Deserialize, Validate, ToSchema)]
#[validate(schema(function = "not_in_the_future"))]
pub struct Purge {
    pub tenant_id: Uuid,
    pub before: DateTime<Utc>,
}

fn not_in_the_future(purge: &Purge) -> Result<(), ValidationError> {
    (purge.before <= Utc::now())
        .then_some(())
        .ok_or_else(|| ValidationError::new("before cannot be in the future"))
}

impl IntoResponse for Hold {
    fn into_response(self) -> Response {
        axum::Json(self).into_response()
    }
}

impl IntoResponse for Released {
    fn into_response(self) -> Response {
        axum::Json(self).into_response()
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(hold)).routes(routes!(release)).routes(routes!(purge))
}

/// Hold a slot: checked like a booking, its places are kept from everyone else for 10 minutes.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/reservations",
    tag = "reservations",
    params(("tenant_id" = Uuid, Path)),
    request_body = Reserve,
    responses((status = 200, body = Hold), (status = 400), (status = 409)),
)]
async fn hold(State(ctx): State<AppContext>, Input(request): Input<Reserve>) -> ApiResult<Hold> {
    let mut transaction = ctx.pool.begin().await?;
    lock(&mut transaction, request.tenant_id).await?;
    let places = fit_slot(&mut transaction, request.tenant_id, &request.parties, None).await?;
    let hold = Hold { id: Uuid::now_v7(), expires_at: Utc::now() + HOLD_FOR };
    take(&mut transaction, request.tenant_id, hold.id, &places, Some(hold.expires_at)).await?;
    transaction.commit().await?;
    Ok(hold)
}

/// Release a hold before it expires.
#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/reservations/{id}",
    tag = "reservations",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = Released), (status = 404)),
)]
async fn release(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Released> {
    let mut transaction = ctx.pool.begin().await?;
    let places = give_back(&mut transaction, key.tenant_id, key.id).await?;
    transaction.commit().await?;
    match places {
        0 => Err(ApiError::NotFound),
        places => Ok(Released { places }),
    }
}

/// Delete the places of expired holds and of times that ended before `before`; run it now and then.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/reservations:purge",
    tag = "reservations",
    params(("tenant_id" = Uuid, Path)),
    request_body = Purge,
    responses((status = 200, body = Released)),
)]
async fn purge(State(ctx): State<AppContext>, Input(request): Input<Purge>) -> ApiResult<Released> {
    let deleted = Db::query("DELETE FROM reservations WHERE tenant_id = $1 AND (upper(during) <= $2 OR expires_at <= now())").sqlite("DELETE FROM reservations WHERE tenant_id = $1 AND (unixepoch(ends_at, 'subsec') <= unixepoch($2, 'subsec') OR unixepoch(expires_at, 'subsec') <= unixepoch('now', 'subsec'))")
        .bind(request.tenant_id)
        .bind(request.before)
        .execute(&ctx.pool)
        .await?;
    Ok(Released { places: deleted.rows_affected() })
}

/// Bookings and holds of one tenant wait for each other until their transaction ends, so two cannot take one place.
pub(super) async fn lock(conn: &mut Tx, tenant_id: Uuid) -> ApiResult<()> {
    Db::query("SELECT pg_advisory_xact_lock(hashtextextended($1::text, 0))")
        .sqlite("SELECT $1")
        .bind(tenant_id)
        .execute(conn)
        .await?;
    Ok(())
}

/// One row per place `owner` takes; a hold's rows expire at `expires_at`.
pub(super) async fn take(
    conn: &mut Tx,
    tenant_id: Uuid,
    owner: Uuid,
    places: &[Place],
    expires_at: Option<DateTime<Utc>>,
) -> ApiResult<()> {
    let (participants, starts, ends, waitlist): (Vec<_>, Vec<_>, Vec<_>, Vec<_>) =
        places.iter().map(|(id, (start, end), waitlist)| (*id, *start, *end, *waitlist)).multiunzip();
    Db::query(
        "INSERT INTO reservations (tenant_id, id, owner_id, participant_id, during, waitlist, expires_at) \
         SELECT $1, uuidv7(), $2, participant, tstzrange(starts, ends), waitlist, $7 \
         FROM unnest($3::uuid[], $4::timestamptz[], $5::timestamptz[], $6::bool[]) AS t(participant, starts, ends, waitlist)",
    )
    .sqlite(
        "INSERT INTO reservations (tenant_id, owner_id, participant_id, starts_at, ends_at, waitlist, expires_at) \
         SELECT $1, $2, unhex(replace(p.value, '-', '')), s.value, e.value, w.value, $7 \
         FROM json_each($3) p JOIN json_each($4) s ON s.key = p.key JOIN json_each($5) e ON e.key = p.key JOIN json_each($6) w ON w.key = p.key",
    )
    .bind(tenant_id)
    .bind(owner)
    .bind(Array(participants))
    .bind(Array(starts))
    .bind(Array(ends))
    .bind(Array(waitlist))
    .bind(expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Gives back every place `owner` takes; how many.
pub(super) async fn give_back(conn: &mut Tx, tenant_id: Uuid, owner: Uuid) -> ApiResult<u64> {
    let deleted = Db::query("DELETE FROM reservations WHERE tenant_id = $1 AND owner_id = $2")
        .bind(tenant_id)
        .bind(owner)
        .execute(conn)
        .await?;
    Ok(deleted.rows_affected())
}

/// Takes `places` out of the unexpired hold `hold`, one held row each; a conflict unless it holds them all.
pub(super) async fn claim(conn: &mut Tx, tenant_id: Uuid, hold: Uuid, places: &[Place]) -> ApiResult<()> {
    let (participants, starts, ends): (Vec<_>, Vec<_>, Vec<_>) = places.iter().map(|(id, (start, end), _)| (*id, *start, *end)).multiunzip();
    let claimed = Db::query(
        "WITH wanted AS (SELECT * FROM unnest($3::uuid[], $4::timestamptz[], $5::timestamptz[]) WITH ORDINALITY AS t(participant, starts, ends, n)), \
         held AS (SELECT DISTINCT ON (w.n) r.id FROM wanted w JOIN reservations r ON r.tenant_id = $1 AND r.owner_id = $2 \
                  AND r.participant_id = w.participant AND r.during = tstzrange(w.starts, w.ends) AND r.expires_at > now() \
                  ORDER BY w.n, r.id) \
         DELETE FROM reservations WHERE tenant_id = $1 AND id IN (SELECT id FROM held)",
    )
    .sqlite(
        "WITH wanted AS (SELECT unhex(replace(p.value, '-', '')) AS participant, s.value AS starts, e.value AS ends \
                         FROM json_each($3) p JOIN json_each($4) s ON s.key = p.key JOIN json_each($5) e ON e.key = p.key), \
         held AS (SELECT (SELECT r.id FROM reservations r WHERE r.tenant_id = $1 AND r.owner_id = $2 AND r.participant_id = w.participant \
                          AND unixepoch(r.starts_at, 'subsec') = unixepoch(w.starts, 'subsec') AND unixepoch(r.ends_at, 'subsec') = unixepoch(w.ends, 'subsec') \
                          AND unixepoch(r.expires_at, 'subsec') > unixepoch('now', 'subsec') ORDER BY r.id LIMIT 1) AS id FROM wanted w) \
         DELETE FROM reservations WHERE tenant_id = $1 AND id IN (SELECT id FROM held)",
    )
    .bind(tenant_id)
    .bind(hold)
    .bind(Array(participants))
    .bind(Array(starts))
    .bind(Array(ends))
    .execute(conn)
    .await?;
    (claimed.rows_affected() == places.len() as u64)
        .then_some(())
        .ok_or_else(|| ApiError::Conflict("the reservation does not hold this time, or it expired".to_string()))
}
