//! Slots that satisfy a search: for every start on the `slot_interval` grid, every way to place the parties and their
//! services (each party offset by up to `party_offset`, each service up to `service_gap` after the previous one),
//! with no conflict of schedule, blockout, capacity or hold.

use std::time::Duration;

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use sqlx::PgPool;

use super::market::{span, Market, Plan};
use super::{Allocation, Error, Interval, Placement, Price, Result, Search, SearchRequest, Slot, Slots};

const DEFAULT_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// A partly placed search: what is allocated so far, and where the party being placed has got to.
#[derive(Clone)]
struct State {
    allocations: Vec<Allocation>,
    placements: Vec<Placement>,
    price: i64,
    cursor: DateTime<Utc>,
}

fn interval_of(request: &SearchRequest) -> chrono::Duration {
    span(if request.slot_interval.is_zero() { DEFAULT_INTERVAL } else { request.slot_interval })
}

/// 0, one interval, two intervals ... up to `most`.
fn grid(most: Duration, interval: chrono::Duration) -> Vec<chrono::Duration> {
    let steps = most.as_secs() / interval.num_seconds().max(1) as u64;
    (0..=steps).map(|step| interval * step as i32).collect()
}

fn anchors(request: &SearchRequest) -> impl Iterator<Item = DateTime<Utc>> + '_ {
    let step = interval_of(request);
    std::iter::successors(Some(request.within.start), move |previous| Some(*previous + step)).take_while(|start| *start < request.within.end)
}

impl Market {
    /// The ways to place one service starting `gap` after the state's cursor.
    fn service_states(&self, states: Vec<State>, party: usize, plan: &Plan, gaps: &[chrono::Duration]) -> Vec<State> {
        states
            .iter()
            .flat_map(|state| {
                gaps.iter().flat_map(move |gap| {
                    let start = state.cursor + *gap;
                    plan.assignments.iter().filter_map(move |picks| {
                        let placed = self.place(plan, picks, start, &state.allocations)?;
                        let price = Price { micros: placed.price, currency: plan.service.price.currency.clone() };
                        let placement = Placement { party, service: plan.service.id, during: Interval { start, end: placed.end }, price };
                        Some(State {
                            allocations: [state.allocations.clone(), placed.allocations].concat(),
                            placements: [state.placements.clone(), vec![placement]].concat(),
                            price: state.price + placed.price,
                            cursor: placed.end,
                        })
                    })
                })
            })
            .collect()
    }

    fn party_states(&self, states: Vec<State>, party: usize, plans: &[Plan], anchor: DateTime<Utc>, request: &SearchRequest) -> Vec<State> {
        let every = interval_of(request);
        let offsets = if party == 0 { vec![chrono::Duration::zero()] } else { grid(request.party_offset, every) };
        let started: Vec<State> = states.into_iter().flat_map(|state| offsets.iter().map(move |offset| State { cursor: anchor + *offset, ..state.clone() })).collect();
        plans.iter().enumerate().fold(started, |states, (index, plan)| {
            let gaps = if index == 0 { vec![chrono::Duration::zero()] } else { grid(request.service_gap, every) };
            self.service_states(states, party, plan, &gaps)
        })
    }

    fn anchor_slots(&self, plans: &[Vec<Plan>], request: &SearchRequest, anchor: DateTime<Utc>) -> Vec<Slot> {
        let first = State { allocations: vec![], placements: vec![], price: 0, cursor: anchor };
        let done = plans.iter().enumerate().fold(vec![first], |states, (party, plans)| self.party_states(states, party, plans, anchor, request));
        let currency = plans.first().and_then(|party| party.first()).map(|plan| plan.service.price.currency.clone()).unwrap_or_default();
        done.into_iter().map(|state| Slot { allocations: state.allocations, placements: state.placements, price: Price { micros: state.price, currency: currency.clone() } }).collect()
    }
}

async fn zone(pool: &PgPool, request: &SearchRequest) -> Result<Tz> {
    let name: Option<Option<String>> = match request.org {
        Some(org) => sqlx::query_scalar("SELECT timezone FROM organizations WHERE id = $1").bind(org).fetch_optional(pool).await?,
        None => None,
    };
    Ok(name.flatten().and_then(|name| name.parse().ok()).unwrap_or(Tz::UTC))
}

async fn found(pool: &PgPool, request: &SearchRequest) -> Result<Vec<Slot>> {
    let market = Market::load(pool, request).await?;
    let plans: Vec<Vec<Plan>> = request.parties.iter().map(|party| market.plans(party)).collect::<Result<_>>()?;
    let slots = anchors(request).flat_map(|anchor| market.anchor_slots(&plans, request, anchor));
    Ok(slots.take(request.limit.unwrap_or(usize::MAX)).collect())
}

/// The request again, asking for the first slot between the end of its window and `horizon_days` later.
fn after_window(request: &SearchRequest, horizon_days: u32) -> Result<SearchRequest> {
    let invalid = |error: serde_json::Error| Error::Invalid(error.to_string());
    let mut later: SearchRequest = serde_json::from_value(serde_json::to_value(request).map_err(invalid)?).map_err(invalid)?;
    later.within = Interval { start: request.within.end, end: request.within.end + chrono::Duration::days(i64::from(horizon_days)) };
    later.limit = Some(1);
    Ok(later)
}

impl Search for Slots {
    async fn find(&self, request: &SearchRequest) -> Result<Vec<Slot>> {
        found(&self.pool, request).await
    }

    async fn next_day(&self, request: &SearchRequest, horizon_days: u32) -> Result<Option<NaiveDate>> {
        let later = after_window(request, horizon_days)?;
        let first = found(&self.pool, &later).await?.into_iter().next();
        let zone = zone(&self.pool, request).await?;
        Ok(first.and_then(|slot| slot.allocations.iter().map(|a| a.during.start).min()).map(|start| start.with_timezone(&zone).date_naive()))
    }
}
