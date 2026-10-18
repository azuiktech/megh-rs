//! `POST /api/v1/orgs/{org_id}/availability:search`, a custom method (AIP-136): the slots where parties can be served.

use std::time::Duration;

use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use axum_extra::extract::WithRejection;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{json, Value};
use uuid::Uuid;

use super::common::{check_scope, instant, money, span_of, zone_of};
use super::error::ApiError;
use super::BookingState;
use crate::booking::{
    ClientId, Condition, Filter, Interval, Party, Search, SearchRequest, ServiceId, ServiceWant, Slot, Slots,
};

const MOST_PARTIES: usize = 10;
const MOST_DAYS: i64 = 31;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchBody {
    parties: Vec<PartyBody>,
    start_time: String,
    end_time: String,
    party_offset: Option<String>,
    service_gap: Option<String>,
    slot_interval: Option<String>,
    page_size: Option<u32>,
    horizon_days: Option<u32>,
    /// The appointment (or held appointment) being moved: its own time counts for nothing in this search.
    reschedule: Option<Uuid>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PartyBody {
    client_id: Option<ClientId>,
    seats: Option<u32>,
    services: Vec<ServiceBody>,
}

/// `wants` are AIP-160 filters; each must hold for at least one bookable chosen for the service.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceBody {
    service_id: ServiceId,
    wants: Option<Vec<String>>,
}

pub(super) fn routes() -> Router<BookingState> {
    Router::new().route("/api/v1/orgs/{org_id}/availability:search", post(search))
}

fn duration(text: &Option<String>, default: Duration) -> Result<Duration, ApiError> {
    text.as_deref().map_or(Ok(default), span_of)
}

fn service_want(body: &ServiceBody) -> Result<ServiceWant, ApiError> {
    let wants = body.wants.iter().flatten().map(|text| Ok(Box::new(Filter::parse(text)?) as Condition)).collect::<Result<_, ApiError>>()?;
    Ok(ServiceWant { service: body.service_id, wants })
}

fn party(body: &PartyBody) -> Result<Party, ApiError> {
    match (body.seats.unwrap_or(1), body.services.is_empty()) {
        (0, _) => Err(ApiError::invalid("seats must be at least 1")),
        (_, true) => Err(ApiError::invalid("a party needs at least one service")),
        (seats, false) => Ok(Party { client: body.client_id, seats, services: body.services.iter().map(service_want).collect::<Result<_, _>>()? }),
    }
}

fn window(body: &SearchBody, zone: Tz) -> Result<Interval, ApiError> {
    let (start, end) = (instant(&body.start_time, zone)?, instant(&body.end_time, zone)?);
    match (end > start, end - start <= chrono::Duration::days(MOST_DAYS)) {
        (false, _) => Err(ApiError::invalid("end_time must be after start_time")),
        (_, false) => Err(ApiError::invalid(format!("the window is at most {MOST_DAYS} days"))),
        _ => Ok(Interval { start, end }),
    }
}

fn search_request(body: &SearchBody, org: Uuid, zone: Tz) -> Result<SearchRequest, ApiError> {
    if body.parties.is_empty() || body.parties.len() > MOST_PARTIES {
        return Err(ApiError::invalid(format!("give between 1 and {MOST_PARTIES} parties")));
    }
    Ok(SearchRequest {
        org: Some(org),
        parties: body.parties.iter().map(party).collect::<Result<_, _>>()?,
        within: window(body, zone)?,
        party_offset: duration(&body.party_offset, Duration::ZERO)?,
        service_gap: duration(&body.service_gap, Duration::ZERO)?,
        slot_interval: duration(&body.slot_interval, Duration::from_secs(900))?,
        limit: Some(body.page_size.unwrap_or(20).clamp(1, 100) as usize),
    })
}

fn local(time: DateTime<Utc>, zone: Tz) -> DateTime<Tz> {
    time.with_timezone(&zone)
}

fn party_json(index: usize, request: &SearchRequest, slot: &Slot, zone: Tz) -> Value {
    let services: Vec<Value> = slot
        .placements
        .iter()
        .filter(|placement| placement.party == index)
        .map(|placement| json!({"service_id": placement.service, "start_time": local(placement.during.start, zone), "end_time": local(placement.during.end, zone), "price": money(&placement.price)}))
        .collect();
    json!({"client_id": request.parties[index].client, "seats": request.parties[index].seats, "services": services})
}

fn slot_json(slot: &Slot, request: &SearchRequest, zone: Tz) -> Value {
    let starts = slot.allocations.iter().map(|allocation| allocation.during.start).min();
    let ends = slot.allocations.iter().map(|allocation| allocation.during.end).max();
    let allocations: Vec<Value> = slot
        .allocations
        .iter()
        .map(|a| json!({"bookable_id": a.bookable, "start_time": local(a.during.start, zone), "end_time": local(a.during.end, zone), "engagement": a.engagement, "seats": a.seats}))
        .collect();
    json!({
        "start_time": starts.map(|time| local(time, zone)),
        "end_time": ends.map(|time| local(time, zone)),
        "price": money(&slot.price),
        "parties": (0..request.parties.len()).map(|index| party_json(index, request, slot, zone)).collect::<Vec<_>>(),
        "allocations": allocations,
    })
}

async fn search(
    State(state): State<BookingState>,
    WithRejection(Path(org), _): WithRejection<Path<Uuid>, ApiError>,
    WithRejection(Json(body), _): WithRejection<Json<SearchBody>, ApiError>,
) -> Result<Json<Value>, ApiError> {
    let zone = zone_of(&state.pool, org).await?;
    let request = search_request(&body, org, zone)?;
    check_scope(&state.pool, org, &request).await?;
    let slots = Slots::new(state.pool.clone());
    let found = match body.reschedule {
        Some(id) => slots.find_ignoring(&request, id).await?,
        None => slots.find(&request).await?,
    };
    let next = match found.is_empty() {
        true => slots.next_day(&request, body.horizon_days.unwrap_or(180).min(366)).await?,
        false => None,
    };
    let shown: Vec<Value> = found.iter().map(|slot| slot_json(slot, &request, zone)).collect();
    Ok(Json(json!({"slots": shown, "next_available_day": next})))
}
