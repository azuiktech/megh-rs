//! Open slots: times when every party's client, providers and room are free, by schedules, blockouts and
//! occupancies. A slot is not an appointment; it becomes one when it is booked.

use std::collections::HashMap;
use std::iter::{once, successors};
use std::ops::Bound;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, TimeDelta, TimeZone, Utc};
use iso8601::Duration;
use itertools::Itertools;
use rrule::{RRuleSet, Tz};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sqlx::PgConnection;
use sqlx::postgres::types::PgRange;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::appointment::{AppointmentData, Status};
use super::blockout::BlockoutData;
use super::provider::ProviderData;
use super::resource::ResourceData;
use super::schedule::ScheduleData;
use super::service::ServiceData;
use crate::auth::crud::Row;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Input};
use crate::{Entity, Table};

/// What to find room for: each party is one client wanting one service.
#[derive(Debug, Deserialize, Validate, ToSchema)]
#[validate(schema(function = "to_after_from"))]
pub struct Availability {
    pub org_id: Uuid,
    pub from: DateTime<FixedOffset>,
    pub to: DateTime<FixedOffset>,
    #[validate(length(min = 1))]
    pub parties: Vec<Party>,
    /// The longest gap between one client's appointments; zero: back to back.
    #[serde(default)]
    #[schema(value_type = String, example = "PT15M")]
    pub service_gap: Duration,
    /// The longest gap between different clients' start times; zero: together.
    #[serde(default)]
    #[schema(value_type = String, example = "PT30M")]
    pub client_gap: Duration,
}

/// A known client by id, or an unknown one named by a tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ToSchema)]
#[serde(untagged)]
pub enum Client {
    Known { id: Uuid },
    Unknown { tag: String },
}

/// Fields a provider or room must have, such as `{"id": …}` or `{"role": "hygienist"}`.
pub type Fields = Map<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Party {
    pub client: Client,
    pub service_id: Uuid,
    /// One object per provider wanted.
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub providers: Vec<Fields>,
    /// What the room must have.
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub resources: Vec<Fields>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct Slots {
    pub slots: Vec<Slot>,
    /// The first day with a slot, when the window has none.
    pub next_available_day: Option<NaiveDate>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct Slot {
    pub parties: Vec<Placed>,
}

/// A party with its providers, room and time filled in.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct Placed {
    pub client: Client,
    pub service_id: Uuid,
    pub providers: Vec<Id>,
    pub resources: Vec<Id>,
    pub starts_at: DateTime<FixedOffset>,
    pub ends_at: DateTime<FixedOffset>,
    /// Only a waitlist place is free.
    pub waitlist: bool,
    /// Free places left at this time: the fewest among the room and providers.
    #[serde(default)]
    pub capacity: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub struct Id {
    pub id: Uuid,
}

impl IntoResponse for Slots {
    fn into_response(self) -> Response {
        axum::Json(self).into_response()
    }
}

fn to_after_from(request: &Availability) -> Result<(), ValidationError> {
    (request.from < request.to)
        .then_some(())
        .ok_or_else(|| ValidationError::new("to must be after from"))
}

/// Start times and gaps are tried on this grid.
const GRID: TimeDelta = TimeDelta::minutes(5);
/// How far `next_available_day` looks.
const LOOK_AHEAD: TimeDelta = TimeDelta::days(180);

type Span = (DateTime<Utc>, DateTime<Utc>);
type Stretch = (TimeDelta, TimeDelta);

/// How long one service takes each participant, from the client's start.
struct Plan {
    client: TimeDelta,
    room: Stretch,
    roles: Vec<(String, Vec<Stretch>)>,
}

impl Plan {
    fn new(service: &ServiceData) -> Self {
        let steps: Vec<_> = service
            .steps
            .iter()
            .scan(TimeDelta::zero(), |at, step| {
                let start = *at;
                *at += length(step.duration);
                Some(((start, *at), step))
            })
            .collect();
        let client = steps.last().map_or(TimeDelta::zero(), |((_, end), _)| *end);
        let roles = steps
            .iter()
            .flat_map(|(stretch, step)| [&step.provider1, &step.provider2].into_iter().flatten().map(|role| (role.clone(), *stretch)))
            .into_group_map()
            .into_iter()
            .sorted()
            .collect();
        Plan { client, room: (-length(service.setup), client + length(service.cleanup)), roles }
    }
}

fn length(duration: Duration) -> TimeDelta {
    TimeDelta::from_std(duration.into()).unwrap_or(TimeDelta::MAX)
}

/// Who takes time: a provider, room or known client by id, or an unknown client by tag.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Who {
    Id(Uuid),
    Tag(String),
}

impl From<&Client> for Who {
    fn from(client: &Client) -> Self {
        match client {
            Client::Known { id } => Who::Id(*id),
            Client::Unknown { tag } => Who::Tag(tag.clone()),
        }
    }
}

#[derive(Debug, Clone)]
struct Taken {
    span: Span,
    waitlist: bool,
}

/// Everything one search reads, loaded once.
struct Calendar {
    plans: HashMap<Uuid, Plan>,
    providers: Vec<Entity<Uuid, ProviderData>>,
    resources: Vec<Entity<Uuid, ResourceData>>,
    fields: HashMap<Uuid, Value>,
    limits: HashMap<Uuid, (i32, i32)>,
    schedules: Vec<(ScheduleData, Vec<Span>)>,
    blockouts: Vec<(BlockoutData, Vec<Span>)>,
    taken: HashMap<Uuid, Vec<Taken>>,
    /// Each party's rooms and crews.
    choices: Vec<(Vec<Uuid>, Vec<Vec<Uuid>>)>,
    /// Open time, merged and sorted, of a room with a crew and of a provider in a room, for a service.
    openings: HashMap<Opening, Vec<Span>>,
}

/// Service, room, crew, and the provider whose open time it is, or none for the room's.
type Opening = (Uuid, Uuid, Vec<Uuid>, Option<Uuid>);

/// Fits a party: whether only a waitlist place is free, the free places left, and the time it takes.
struct Fit {
    waitlist: bool,
    capacity: i32,
    taken: Vec<(Who, Taken)>,
}

impl Calendar {
    /// `except` leaves one appointment's time out, for booking it again.
    async fn load(conn: &mut PgConnection, request: &Availability, window: Span, except: Option<Uuid>) -> ApiResult<Self> {
        let horizon = (window.0 - TimeDelta::days(1), window.1 + TimeDelta::days(1));
        let services: Vec<Uuid> = request.parties.iter().map(|party| party.service_id).unique().collect();
        let plans: HashMap<Uuid, Plan> = services_of(conn, request.org_id, &services)
            .await?
            .iter()
            .map(|service| (service.id, Plan::new(&service.data)))
            .collect();
        if let Some(party) = request.parties.iter().find(|party| !plans.contains_key(&party.service_id)) {
            return Err(ApiError::BadRequest(format!("no service {}", party.service_id)));
        }
        let roles: Vec<String> = plans.values().flat_map(|plan| plan.roles.iter().map(|(role, _)| role.clone())).unique().collect();
        let providers = providers_in(conn, request.org_id, &roles).await?;
        let resources = rooms_for(conn, request).await?;
        let (fields, limits) = described(&providers, &resources);
        let (provider_ids, room_ids): (Vec<Uuid>, Vec<Uuid>) = (providers.iter().map(|p| p.id).collect(), resources.iter().map(|r| r.id).collect());
        let candidates = Candidates { org_id: request.org_id, providers: &provider_ids, rooms: &room_ids, services: &services };
        let schedules = applying::<ScheduleData>(conn, &candidates).await?.into_iter().map(|s| {
            let open = windows(&s.data.recurrence, s.data.starts_at, s.data.ends_at, horizon);
            (s.data, open)
        });
        let blockouts = applying::<BlockoutData>(conn, &candidates).await?.into_iter().map(|b| {
            let closed = windows(&b.data.recurrence, b.data.starts_at, b.data.ends_at, horizon);
            (b.data, closed)
        });
        let clients = request.parties.iter().filter_map(|party| match party.client {
            Client::Known { id } => Some(id),
            Client::Unknown { .. } => None,
        });
        let participants: Vec<Uuid> = provider_ids.iter().chain(&room_ids).copied().chain(clients).collect();
        let mut calendar = Calendar {
            plans,
            providers,
            resources,
            fields,
            limits,
            schedules: schedules.collect(),
            blockouts: blockouts.collect(),
            taken: taken(conn, request.org_id, &participants, horizon, except).await?,
            choices: Vec::new(),
            openings: HashMap::new(),
        };
        calendar.choices = request.parties.iter().map(|party| (calendar.rooms(party), calendar.crews(party))).collect();
        calendar.openings = calendar.openings(request);
        Ok(calendar)
    }

    /// The open time of every room with every crew and of every provider in every room, for each party's service.
    fn openings(&self, request: &Availability) -> HashMap<Opening, Vec<Span>> {
        request
            .parties
            .iter()
            .zip(&self.choices)
            .flat_map(|(party, (rooms, crews))| {
                rooms.iter().cartesian_product(crews).flat_map(move |(room, crew)| {
                    once(None).chain(crew.iter().copied().map(Some)).map(move |provider| (party.service_id, *room, crew.clone(), provider))
                })
            })
            .unique()
            .map(|key| {
                let open = self.open_time(&key);
                (key, open)
            })
            .collect()
    }

    /// Schedule windows minus blockout windows of this service, room and crew; with a provider, of that provider.
    fn open_time(&self, (service, room, crew, provider): &Opening) -> Vec<Span> {
        let scheduled = self
            .schedules
            .iter()
            .filter(|(s, _)| {
                serves(&s.service_ids, *service)
                    && s.resource_id.is_none_or(|r| r == *room)
                    && s.provider_id.is_none_or(|p| provider.map_or(crew.contains(&p), |x| x == p))
            })
            .flat_map(|(_, open)| open.iter().copied());
        let blocked = self
            .blockouts
            .iter()
            .filter(|(b, _)| serves(&b.service_ids, *service) && blocks(b, *room, *provider))
            .flat_map(|(_, closed)| closed.iter().copied());
        subtract(merge(scheduled), &merge(blocked))
    }

    /// Every start time the first party's service is scheduled for, one room time (setup, client time and
    /// cleanup) apart, so that consecutive slots can share a room.
    fn starts(&self, party: &Party, window: Span) -> Vec<DateTime<Utc>> {
        let plan = &self.plans[&party.service_id];
        let step = (plan.room.1 - plan.room.0).max(GRID);
        self.schedules
            .iter()
            .filter(|(schedule, _)| serves(&schedule.service_ids, party.service_id))
            .flat_map(|(_, open)| open.iter())
            .flat_map(|open| successors(Some(open.0 - plan.room.0), move |at| Some(*at + step)).take_while(move |at| *at + plan.client <= open.1))
            .filter(|at| window.0 <= *at && *at + plan.client <= window.1)
            .sorted()
            .dedup()
            .collect()
    }

    fn rooms(&self, party: &Party) -> Vec<Uuid> {
        self.resources
            .iter()
            .filter(|room| party.resources.iter().all(|wanted| matches(&self.fields[&room.id], wanted)))
            .map(|room| room.id)
            .collect()
    }

    /// One provider per role of the service, all different, together matching every provider wanted.
    fn crews(&self, party: &Party) -> Vec<Vec<Uuid>> {
        self.plans[&party.service_id]
            .roles
            .iter()
            .map(|(role, _)| self.providers.iter().filter(|p| p.data.role.as_ref().is_some_and(|r| r.as_ref() == role)).map(|p| p.id).collect::<Vec<_>>())
            .multi_cartesian_product()
            .filter(|crew| crew.iter().all_unique())
            .filter(|crew| party.providers.iter().all(|wanted| crew.iter().any(|id| matches(&self.fields[id], wanted))))
            .collect()
    }

    /// Whether `placed` still fits as given, after the places `held`.
    fn check(&self, party: &Party, placed: &Placed, held: &[(Who, Taken)]) -> ApiResult<Fit> {
        let bad = |reason: &str| ApiError::BadRequest(reason.to_string());
        let start = placed.starts_at.to_utc();
        let length = self.plans[&party.service_id].client;
        (start + length == placed.ends_at.to_utc()).then_some(()).ok_or_else(|| bad("ends_at must be starts_at plus the service's time"))?;
        let provider_ids: Vec<Uuid> = placed.providers.iter().map(|provider| provider.id).collect();
        let crew = self.crew(party.service_id, &provider_ids).ok_or_else(|| bad("providers must be one per role of the service"))?;
        let room = placed.resources[0].id;
        self.place(party, start, room, &crew, held, &[placed.waitlist])
            .ok_or_else(|| ApiError::Conflict(format!("the time is not open for a {} place", if placed.waitlist { "waitlist" } else { "free" })))
    }

    /// `provider_ids` as a crew, one per role of the service in the service's order.
    fn crew(&self, service: Uuid, provider_ids: &[Uuid]) -> Option<Vec<Uuid>> {
        let roles = &self.plans[&service].roles;
        let role_of = |id: &Uuid| self.providers.iter().find(|p| p.id == *id).and_then(|p| p.data.role.as_ref());
        let crew = roles
            .iter()
            .map(|(role, _)| provider_ids.iter().copied().find(|id| role_of(id).is_some_and(|r| r.as_ref() == role)))
            .collect::<Option<Vec<_>>>()?;
        (crew.len() == provider_ids.len() && crew.iter().all_unique()).then_some(crew)
    }

    /// The time the party's client, room and crew take from `start`, if all are open and have a place of the
    /// first of `waitlist` kinds that fits: a free place (`false`) or a waitlist place (`true`).
    fn place(&self, party: &Party, start: DateTime<Utc>, room: Uuid, crew: &[Uuid], held: &[(Who, Taken)], waitlist: &[bool]) -> Option<Fit> {
        let plan = &self.plans[&party.service_id];
        let client = (Who::from(&party.client), (start, start + plan.client));
        let room_span = (start + plan.room.0, start + plan.room.1);
        let staff: Vec<(Uuid, Span)> = plan
            .roles
            .iter()
            .zip(crew)
            .flat_map(|((_, stretches), id)| stretches.iter().map(move |s| (*id, (start + s.0, start + s.1))))
            .collect();
        let open_for = |provider: Option<Uuid>, span: Span| {
            self.openings.get(&(party.service_id, room, crew.to_vec(), provider)).is_some_and(|open| inside(open, span))
        };
        let open = open_for(None, room_span) && staff.iter().all(|(id, span)| open_for(Some(*id), *span));
        let client_free = self.busy(&client.0, client.1, held, None) < 1;
        (open && client_free).then_some(())?;
        let others: Vec<(Who, Span)> = once((Who::Id(room), room_span)).chain(staff.iter().map(|(id, span)| (Who::Id(*id), *span))).collect();
        let has_place = |waitlist: bool| {
            others.iter().all(|(who, span)| self.busy(who, *span, held, Some(waitlist)) < self.limit(who, waitlist))
        };
        let waitlist = waitlist.iter().copied().find(|waitlist| has_place(*waitlist))?;
        let capacity = others.iter().map(|(who, span)| self.limit(who, false) - self.busy(who, *span, held, Some(false))).min().unwrap_or(0).max(0);
        let taken = once(client).chain(others).map(|(who, span)| (who, Taken { span, waitlist })).collect();
        Some(Fit { waitlist, capacity, taken })
    }

    fn limit(&self, who: &Who, waitlist: bool) -> i32 {
        let (capacity, places) = match who {
            Who::Id(id) => self.limits.get(id).copied().unwrap_or((1, 0)),
            Who::Tag(_) => (1, 0),
        };
        if waitlist { places } else { capacity }
    }

    /// The most appointments that take `who` at once during `span`, of the given kind or of both.
    fn busy(&self, who: &Who, span: Span, held: &[(Who, Taken)], waitlist: Option<bool>) -> i32 {
        let stored = match who {
            Who::Id(id) => self.taken.get(id).map(Vec::as_slice).unwrap_or_default(),
            Who::Tag(_) => &[],
        };
        let spans = stored
            .iter()
            .chain(held.iter().filter(|(holder, _)| holder == who).map(|(_, taken)| taken))
            .filter(|taken| waitlist.is_none_or(|kind| taken.waitlist == kind))
            .map(|taken| taken.span);
        most_at_once(spans, span)
    }
}

/// Each provider's and room's fields as JSON, for matching, and its capacity and waitlist.
fn described(
    providers: &[Entity<Uuid, ProviderData>],
    resources: &[Entity<Uuid, ResourceData>],
) -> (HashMap<Uuid, Value>, HashMap<Uuid, (i32, i32)>) {
    let providers = providers.iter().map(|p| (p.id, to_json(p), (p.data.capacity.into(), p.data.waitlist.into())));
    let resources = resources.iter().map(|r| (r.id, to_json(r), (r.data.capacity.into(), r.data.waitlist.into())));
    providers.chain(resources).map(|(id, fields, limits)| ((id, fields), (id, limits))).unzip()
}

/// One search: the request and its window.
struct Search<'a> {
    calendar: &'a Calendar,
    request: &'a Availability,
    window: Span,
}

impl<'a> Search<'a> {
    fn new(calendar: &'a Calendar, request: &'a Availability, window: Span) -> Self {
        Search { calendar, request, window }
    }

    /// One slot per start time of the first party, earliest first.
    fn slots(&self) -> impl Iterator<Item = Slot> + '_ {
        self.calendar
            .starts(&self.request.parties[0], self.window)
            .into_iter()
            .filter_map(|start| self.extend(start, Vec::new(), Vec::new()))
            .map(|parties| Slot { parties })
    }

    /// Places the next party after `placed`, trying its start times, rooms and crews in order; a free place
    /// before a waitlist place.
    fn extend(&self, first: DateTime<Utc>, placed: Vec<Placed>, held: Vec<(Who, Taken)>) -> Option<Vec<Placed>> {
        let Some(party) = self.request.parties.get(placed.len()) else {
            return Some(placed);
        };
        let plan = &self.calendar.plans[&party.service_id];
        let (rooms, crews) = &self.calendar.choices[placed.len()];
        let options: Vec<_> = self
            .start_options(party, first, &placed)
            .into_iter()
            .filter(|start| self.window.0 <= *start && *start + plan.client <= self.window.1)
            .cartesian_product(rooms.iter())
            .cartesian_product(crews.iter())
            .collect();
        let mut free_first = [false, true].into_iter().flat_map(|waitlist_allowed| options.iter().map(move |option| (waitlist_allowed, option)));
        free_first.find_map(|(waitlist_allowed, ((start, room), crew))| {
            let fit = self.calendar.place(party, *start, **room, crew, &held, &[false, true])?;
            (waitlist_allowed || !fit.waitlist).then_some(())?;
            let next = self.placed(party, *start, **room, crew, &fit);
            self.extend(first, placed.iter().cloned().chain(once(next)).collect(), held.iter().cloned().chain(fit.taken).collect())
        })
    }

    fn placed(&self, party: &Party, start: DateTime<Utc>, room: Uuid, crew: &[Uuid], fit: &Fit) -> Placed {
        let offset = self.request.from.timezone();
        Placed {
            client: party.client.clone(),
            service_id: party.service_id,
            providers: crew.iter().map(|id| Id { id: *id }).collect(),
            resources: vec![Id { id: room }],
            starts_at: start.with_timezone(&offset),
            ends_at: (start + self.calendar.plans[&party.service_id].client).with_timezone(&offset),
            waitlist: fit.waitlist,
            capacity: fit.capacity,
        }
    }

    /// The first party starts at `first`; a client's next service up to `service_gap` after its last; another
    /// client up to `client_gap` after `first`.
    fn start_options(&self, party: &Party, first: DateTime<Utc>, placed: &[Placed]) -> Vec<DateTime<Utc>> {
        if placed.is_empty() {
            return vec![first];
        }
        let (from, gap) = placed
            .iter()
            .rev()
            .find(|earlier| earlier.client == party.client)
            .map_or((first, self.request.client_gap), |earlier| (earlier.ends_at.to_utc(), self.request.service_gap));
        let last = from + length(gap);
        successors(Some(from), |at| Some(*at + GRID)).take_while(|at| *at <= last).collect()
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(available))
}

/// Open slots for every party together; booking a slot makes its appointments.
#[utoipa::path(
    post,
    path = "/orgs/{org_id}/appointments:available",
    tag = "appointments",
    params(("org_id" = Uuid, Path)),
    request_body = Availability,
    responses((status = 200, body = Slots), (status = 400)),
)]
async fn available(State(ctx): State<AppContext>, Input(request): Input<Availability>) -> ApiResult<Slots> {
    let unknown = request.parties.iter().find_map(|party| {
        unknown_field::<ProviderData>(&party.providers).or_else(|| unknown_field::<ResourceData>(&party.resources))
    });
    if let Some(field) = unknown {
        return Err(ApiError::BadRequest(format!("{field} is not a field; attributes cannot be matched yet")));
    }
    let window = (request.from.to_utc(), request.to.to_utc());
    let mut conn = ctx.pool.acquire().await?;
    let calendar = Calendar::load(&mut conn, &request, window, None).await?;
    let slots: Vec<Slot> = Search::new(&calendar, &request, window).slots().collect();
    let next_available_day = match slots.is_empty() {
        true => next_available_day(&mut conn, &request, window.1).await?,
        false => None,
    };
    Ok(Slots { slots, next_available_day })
}

async fn next_available_day(conn: &mut PgConnection, request: &Availability, after: DateTime<Utc>) -> ApiResult<Option<NaiveDate>> {
    let window = (after, after + LOOK_AHEAD);
    let calendar = Calendar::load(conn, request, window, None).await?;
    let first = Search::new(&calendar, request, window).slots().next();
    Ok(first.map(|slot| slot.parties[0].starts_at.date_naive()))
}

/// One place taken: the participant, when, and whether it is a waitlist place.
pub(super) type Place = (Uuid, Span, bool);

/// The places `appointment` takes, read in the caller's transaction, if it fits as its status asks: `booked`
/// needs a free place, `waitlist` a waitlist place. `except` leaves out the places of the appointment being
/// booked again, or of the hold it is booked from.
pub(super) async fn fit(conn: &mut PgConnection, appointment: &AppointmentData, except: Option<Uuid>) -> ApiResult<Vec<Place>> {
    let party = Placed {
        client: Client::Known { id: appointment.client_id },
        service_id: appointment.service_id,
        providers: appointment.provider_ids.iter().map(|id| Id { id: *id }).collect(),
        resources: vec![Id { id: appointment.resource_id }],
        starts_at: appointment.starts_at.fixed_offset(),
        ends_at: appointment.ends_at.fixed_offset(),
        waitlist: appointment.status == Status::Waitlist,
        capacity: 0,
    };
    fit_slot(conn, appointment.org_id, &[party], except).await
}

/// The places a slot's parties take, if each still fits as given (a free place, or a waitlist place when
/// `waitlist`), counting the parties before it.
pub(super) async fn fit_slot(conn: &mut PgConnection, org_id: Uuid, parties: &[Placed], except: Option<Uuid>) -> ApiResult<Vec<Place>> {
    let bad = |reason: &str| ApiError::BadRequest(reason.to_string());
    let from = parties.iter().map(|placed| placed.starts_at).min().ok_or_else(|| bad("a slot has parties"))?;
    let to = parties.iter().map(|placed| placed.ends_at).max().ok_or_else(|| bad("a slot has parties"))?;
    let request = Availability {
        org_id,
        from,
        to,
        parties: parties.iter().map(searched).collect::<ApiResult<_>>()?,
        service_gap: Duration::default(),
        client_gap: Duration::default(),
    };
    let calendar = Calendar::load(conn, &request, (from.to_utc(), to.to_utc()), except).await?;
    let held = parties.iter().zip(&request.parties).try_fold(Vec::new(), |held: Vec<(Who, Taken)>, (placed, party)| {
        let fit = calendar.check(party, placed, &held)?;
        Ok::<_, ApiError>(held.into_iter().chain(fit.taken).collect())
    })?;
    Ok(held
        .into_iter()
        .filter_map(|(who, taken)| match who {
            Who::Id(id) => Some((id, taken.span, taken.waitlist)),
            Who::Tag(_) => None,
        })
        .collect())
}

/// A placed party as the search reads it: its one room by id.
fn searched(placed: &Placed) -> ApiResult<Party> {
    let [room] = placed.resources.as_slice() else {
        return Err(ApiError::BadRequest("a party has one room".to_string()));
    };
    Ok(Party {
        client: placed.client.clone(),
        service_id: placed.service_id,
        providers: Vec::new(),
        resources: vec![Map::from_iter([("id".to_string(), Value::String(room.id.to_string()))])],
    })
}

/// The ids a search can use, for the queries that read only what applies to them.
struct Candidates<'a> {
    org_id: Uuid,
    providers: &'a [Uuid],
    rooms: &'a [Uuid],
    services: &'a [Uuid],
}

async fn services_of(conn: &mut PgConnection, org_id: Uuid, ids: &[Uuid]) -> ApiResult<Vec<Entity<Uuid, ServiceData>>> {
    Ok(sqlx::query_as("SELECT * FROM services WHERE org_id = $1 AND id = ANY($2) AND deleted_at IS NULL")
        .bind(org_id)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

/// Providers whose role a step of the services needs, e.g. every hygienist.
async fn providers_in(conn: &mut PgConnection, org_id: Uuid, roles: &[String]) -> ApiResult<Vec<Entity<Uuid, ProviderData>>> {
    Ok(sqlx::query_as("SELECT * FROM providers WHERE org_id = $1 AND role = ANY($2) AND deleted_at IS NULL ORDER BY id")
        .bind(org_id)
        .bind(roles)
        .fetch_all(conn)
        .await?)
}

/// Rooms with every field some party asks of its room.
async fn rooms_for(conn: &mut PgConnection, request: &Availability) -> ApiResult<Vec<Entity<Uuid, ResourceData>>> {
    let wanted: Vec<Value> = request.parties.iter().map(|party| Value::Object(party.resources.iter().flatten().map(|(k, v)| (k.clone(), v.clone())).collect())).collect();
    Ok(sqlx::query_as(
        "SELECT * FROM resources r WHERE org_id = $1 AND deleted_at IS NULL \
         AND EXISTS (SELECT 1 FROM unnest($2::jsonb[]) wanted WHERE to_jsonb(r) @> wanted) ORDER BY id",
    )
    .bind(request.org_id)
    .bind(wanted)
    .fetch_all(conn)
    .await?)
}

/// Schedules or blockouts that apply to the candidates and services: naming none of them means any.
async fn applying<T: Row>(conn: &mut PgConnection, candidates: &Candidates<'_>) -> ApiResult<Vec<Entity<Uuid, T>>> {
    let query = format!(
        "SELECT * FROM {} WHERE org_id = $1 AND deleted_at IS NULL \
         AND (provider_id IS NULL OR provider_id = ANY($2)) AND (resource_id IS NULL OR resource_id = ANY($3)) \
         AND (cardinality(service_ids) = 0 OR service_ids && $4)",
        T::TABLE_NAME
    );
    Ok(sqlx::query_as(&query)
        .bind(candidates.org_id)
        .bind(candidates.providers)
        .bind(candidates.rooms)
        .bind(candidates.services)
        .fetch_all(conn)
        .await?)
}

/// The places taken from the participants in `horizon`: appointments and unexpired holds, except `except`'s.
async fn taken(
    conn: &mut PgConnection,
    org_id: Uuid,
    participants: &[Uuid],
    horizon: Span,
    except: Option<Uuid>,
) -> ApiResult<HashMap<Uuid, Vec<Taken>>> {
    let rows: Vec<(Uuid, PgRange<DateTime<Utc>>, bool)> = sqlx::query_as(
        "SELECT participant_id, during, waitlist FROM reservations \
         WHERE org_id = $1 AND participant_id = ANY($2) AND during && $3 \
         AND (expires_at IS NULL OR expires_at > now()) AND owner_id IS DISTINCT FROM $4",
    )
    .bind(org_id)
    .bind(participants)
    .bind(PgRange::from(horizon.0..horizon.1))
    .bind(except)
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(who, during, waitlist)| span(&during).map(|span| (who, Taken { span, waitlist })))
        .into_group_map())
}

fn span(range: &PgRange<DateTime<Utc>>) -> Option<Span> {
    match (range.start, range.end) {
        (Bound::Included(start) | Bound::Excluded(start), Bound::Included(end) | Bound::Excluded(end)) => Some((start, end)),
        _ => None,
    }
}

/// The windows a recurrence gives inside `horizon`, from `starts_at` to `ends_at` on each day, in its time zone.
fn windows(recurrence: &RRuleSet, starts_at: NaiveTime, ends_at: NaiveTime, horizon: Span) -> Vec<Span> {
    let tz = recurrence.get_dt_start().timezone();
    let day = TimeDelta::days(1);
    recurrence
        .clone()
        .after((horizon.0 - day).with_timezone(&tz))
        .before((horizon.1 + day).with_timezone(&tz))
        .all(u16::MAX)
        .dates
        .iter()
        .filter_map(|day| local_span(day.timezone(), day.date_naive(), starts_at, ends_at))
        .collect()
}

fn local_span(tz: Tz, date: NaiveDate, starts_at: NaiveTime, ends_at: NaiveTime) -> Option<Span> {
    let at = |time: NaiveTime| tz.from_local_datetime(&date.and_time(time)).earliest().map(|t| t.with_timezone(&Utc));
    Some((at(starts_at)?, at(ends_at)?))
}

fn to_json<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or_default()
}

fn matches(fields: &Value, wanted: &Fields) -> bool {
    wanted.iter().all(|(field, value)| fields.get(field) == Some(value))
}

/// A wanted field that is not a column, or is `attributes` (matching attributes needs a GIN index, later).
fn unknown_field<T: Table>(wanted: &[Fields]) -> Option<String> {
    let is_field = |field: &str| field == "id" || (field != "attributes" && T::COLUMNS.iter().any(|column| column.name == field));
    wanted.iter().flat_map(Map::keys).find(|field| !is_field(field)).cloned()
}

fn serves(service_ids: &[Uuid], service: Uuid) -> bool {
    service_ids.is_empty() || service_ids.contains(&service)
}

/// A blockout with neither provider nor room closes everything; with a room, that room and the providers in
/// it; with a provider, that provider.
fn blocks(blockout: &BlockoutData, room: Uuid, provider: Option<Uuid>) -> bool {
    let in_room = blockout.resource_id.is_none_or(|r| r == room);
    match provider {
        None => blockout.provider_id.is_none() && in_room,
        Some(provider) => blockout.provider_id.map_or(blockout.resource_id.is_none(), |p| p == provider) && in_room,
    }
}

fn overlaps(a: Span, b: Span) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// Windows sorted and joined where they touch or overlap.
fn merge(windows: impl Iterator<Item = Span>) -> Vec<Span> {
    windows.sorted().coalesce(|a, b| if b.0 <= a.1 { Ok((a.0, a.1.max(b.1))) } else { Err((a, b)) }).collect()
}

/// `open` minus `closed`, both merged; the result stays merged.
fn subtract(open: Vec<Span>, closed: &[Span]) -> Vec<Span> {
    open.into_iter()
        .flat_map(|window| {
            let cuts = closed.iter().filter(move |c| overlaps(**c, window));
            let starts = once(window.0).chain(cuts.clone().map(|c| c.1));
            let ends = cuts.map(|c| c.0).chain(once(window.1));
            starts.zip(ends).filter(|(start, end)| start < end).collect::<Vec<_>>()
        })
        .collect()
}

/// Whether `span` lies inside one window of a merged, sorted list: a binary search.
fn inside(open: &[Span], span: Span) -> bool {
    let i = open.partition_point(|window| window.0 <= span.0);
    i > 0 && open[i - 1].1 >= span.1
}

/// The most spans that overlap at one instant inside `within`.
fn most_at_once(spans: impl Iterator<Item = Span>, within: Span) -> i32 {
    spans
        .filter(|span| overlaps(*span, within))
        .flat_map(|span| [(span.0.max(within.0), 1), (span.1.min(within.1), -1)])
        .sorted_by_key(|(at, change)| (*at, *change))
        .scan(0, |now, (_, change)| {
            *now += change;
            Some(*now)
        })
        .max()
        .unwrap_or(0)
}
