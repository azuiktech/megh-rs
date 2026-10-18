//! A slot posted back by a client is only a claim. The server searches again for exactly those parties and times
//! and uses the slot it finds, so a crafted slot (another org's bookable, a shorter service) is never held.

use std::time::Duration;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use num_integer::Integer;
use serde::Deserialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::common::{check_scope, instant};
use super::error::ApiError;
use crate::booking::{Allocation, BookableId, ClientId, Engagement, Interval, Party, Search, SearchRequest, ServiceId, ServiceWant, Slot, Slots};

/// A slot as `availability:search` returns it (the price and the totals are ignored: the server knows them).
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(deny_unknown_fields)]
pub(super) struct SlotBody {
    start_time: Option<Value>,
    end_time: Option<Value>,
    price: Option<Value>,
    parties: Vec<SlotParty>,
    allocations: Vec<SlotAllocation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotParty {
    client_id: Option<ClientId>,
    seats: Option<u32>,
    services: Vec<SlotService>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(dead_code)]
struct SlotService {
    service_id: ServiceId,
    start_time: String,
    end_time: String,
    price: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SlotAllocation {
    bookable_id: BookableId,
    start_time: String,
    end_time: String,
    engagement: Engagement,
    seats: u32,
}

fn interval(start: &str, end: &str, zone: Tz) -> Result<Interval, ApiError> {
    Ok(Interval { start: instant(start, zone)?, end: instant(end, zone)? })
}

/// Each party's services, with the times the client claims.
fn claimed_services(body: &SlotBody, zone: Tz) -> Result<Vec<Vec<(ServiceId, Interval)>>, ApiError> {
    body.parties.iter().map(|party| party.services.iter().map(|service| Ok((service.service_id, interval(&service.start_time, &service.end_time, zone)?))).collect()).collect()
}

fn claimed_allocations(body: &SlotBody, zone: Tz) -> Result<Vec<Allocation>, ApiError> {
    let one = |a: &SlotAllocation| Ok(Allocation { bookable: a.bookable_id, during: interval(&a.start_time, &a.end_time, zone)?, engagement: a.engagement, seats: a.seats });
    body.allocations.iter().map(one).collect()
}

fn seconds_between(later: DateTime<Utc>, earlier: DateTime<Utc>) -> u64 {
    (later - earlier).num_seconds().unsigned_abs()
}

/// The search that finds the claimed slot first: its window, and the offsets and gaps it shows, on a grid they all fit.
fn request_for(body: &SlotBody, claimed: &[Vec<(ServiceId, Interval)>], org: Uuid) -> Result<SearchRequest, ApiError> {
    let empty = || ApiError::invalid("a slot needs at least one service");
    let starts: Vec<DateTime<Utc>> = claimed.iter().map(|party| party.first().map(|(_, during)| during.start).ok_or_else(empty)).collect::<Result<_, _>>()?;
    let ends = claimed.iter().flatten().map(|(_, during)| during.end);
    let (first, last) = (starts.iter().min().copied().ok_or_else(empty)?, ends.max().ok_or_else(empty)?);
    let party_offset = seconds_between(starts.iter().max().copied().unwrap_or(first), first);
    let gaps = claimed.iter().flat_map(|party| party.windows(2).map(|pair| seconds_between(pair[1].1.start, pair[0].1.end)));
    let service_gap = gaps.clone().max().unwrap_or(0);
    let grid = gaps.chain([party_offset]).filter(|seconds| *seconds > 0).fold(0, |grid: u64, seconds| grid.gcd(&seconds)).max(60);
    let parties = body.parties.iter().map(|party| Party { client: party.client_id, seats: party.seats.unwrap_or(1), services: party.services.iter().map(|s| ServiceWant { service: s.service_id, wants: vec![] }).collect() });
    Ok(SearchRequest {
        org: Some(org),
        parties: parties.collect(),
        within: Interval { start: first, end: last },
        party_offset: Duration::from_secs(party_offset),
        service_gap: Duration::from_secs(service_gap),
        slot_interval: Duration::from_secs(grid),
        limit: None,
    })
}

fn sorted(mut allocations: Vec<Allocation>) -> Vec<Allocation> {
    allocations.sort_by_key(|a| (a.bookable.0, a.during.start, a.during.end));
    allocations
}

fn same_times(slot: &Slot, claimed: &[Vec<(ServiceId, Interval)>]) -> bool {
    claimed.iter().enumerate().all(|(index, party)| {
        let found: Vec<_> = slot.placements.iter().filter(|placement| placement.party == index).map(|placement| (placement.service, placement.during)).collect();
        found == *party
    })
}

/// The search request of the claim, and the server's own slot that matches it, or `Invalid` when there is none.
/// `ignoring` is the appointment being moved, whose own time counts for nothing.
pub(super) async fn verified(pool: &PgPool, org: Uuid, zone: Tz, body: &SlotBody, ignoring: Option<Uuid>) -> Result<(SearchRequest, Slot), ApiError> {
    let (claimed, allocations) = (claimed_services(body, zone)?, claimed_allocations(body, zone)?);
    let request = request_for(body, &claimed, org)?;
    check_scope(pool, org, &request).await?;
    let wanted = sorted(allocations);
    let slots = Slots::new(pool.clone());
    let found = match ignoring {
        Some(id) => slots.find_ignoring(&request, id).await?,
        None => slots.find(&request).await?,
    };
    let slot = found.into_iter().find(|slot| same_times(slot, &claimed) && sorted(slot.allocations.clone()) == wanted);
    slot.map(|slot| (request, slot)).ok_or_else(|| ApiError::invalid("the slot is not available any more; search again"))
}
