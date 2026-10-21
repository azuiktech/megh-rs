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

#[derive(Debug, Serialize, ToSchema)]
pub struct Slot {
    pub parties: Vec<Placed>,
}

/// A party with its providers, room and time filled in.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Placed {
    pub client: Client,
    pub service_id: Uuid,
    pub providers: Vec<Id>,
    pub resources: Vec<Id>,
    pub starts_at: DateTime<FixedOffset>,
    pub ends_at: DateTime<FixedOffset>,
    /// Only a waitlist place is free.
    pub waitlist: bool,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
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
}

impl Calendar {
    async fn load(conn: &mut PgConnection, request: &Availability, window: Span) -> ApiResult<Self> {
        let horizon = (window.0 - TimeDelta::days(1), window.1 + TimeDelta::days(1));
        let plans: HashMap<Uuid, Plan> = rows::<ServiceData>(conn, request.org_id)
            .await?
            .iter()
            .map(|service| (service.id, Plan::new(&service.data)))
            .collect();
        if let Some(party) = request.parties.iter().find(|party| !plans.contains_key(&party.service_id)) {
            return Err(ApiError::BadRequest(format!("no service {}", party.service_id)));
        }
        let providers = rows::<ProviderData>(conn, request.org_id).await?;
        let resources = rows::<ResourceData>(conn, request.org_id).await?;
        let (fields, limits) = described(&providers, &resources);
        let schedules = rows::<ScheduleData>(conn, request.org_id).await?.into_iter().map(|s| {
            let open = windows(&s.data.recurrence, s.data.starts_at, s.data.ends_at, horizon);
            (s.data, open)
        });
        let blockouts = rows::<BlockoutData>(conn, request.org_id).await?.into_iter().map(|b| {
            let closed = windows(&b.data.recurrence, b.data.starts_at, b.data.ends_at, horizon);
            (b.data, closed)
        });
        Ok(Calendar {
            plans,
            providers,
            resources,
            fields,
            limits,
            schedules: schedules.collect(),
            blockouts: blockouts.collect(),
            taken: taken(conn, request.org_id, horizon).await?,
        })
    }

    /// Every start time the first party's service is scheduled for, one service length apart.
    fn starts(&self, party: &Party, window: Span) -> Vec<DateTime<Utc>> {
        let plan = &self.plans[&party.service_id];
        let step = plan.client.max(GRID);
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
    fn place(&self, party: &Party, start: DateTime<Utc>, room: Uuid, crew: &[Uuid], held: &[(Who, Taken)], waitlist: &[bool]) -> Option<(bool, Vec<(Who, Taken)>)> {
        let plan = &self.plans[&party.service_id];
        let client = (Who::from(&party.client), (start, start + plan.client));
        let room_span = (start + plan.room.0, start + plan.room.1);
        let staff: Vec<(Uuid, Span)> = plan
            .roles
            .iter()
            .zip(crew)
            .flat_map(|((_, stretches), id)| stretches.iter().map(move |s| (*id, (start + s.0, start + s.1))))
            .collect();
        let open = self.open(party.service_id, room, crew, None, room_span)
            && staff.iter().all(|(id, span)| self.open(party.service_id, room, crew, Some(*id), *span));
        let client_free = self.busy(&client.0, client.1, held, None) < 1;
        (open && client_free).then_some(())?;
        let others: Vec<(Who, Span)> = once((Who::Id(room), room_span)).chain(staff.iter().map(|(id, span)| (Who::Id(*id), *span))).collect();
        let has_place = |waitlist: bool| {
            others.iter().all(|(who, span)| self.busy(who, *span, held, Some(waitlist)) < self.limit(who, waitlist))
        };
        let waitlist = waitlist.iter().copied().find(|waitlist| has_place(*waitlist))?;
        let taken = once(client).chain(others).map(|(who, span)| (who, Taken { span, waitlist })).collect();
        Some((waitlist, taken))
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

    /// Whether `span` is inside the schedules and outside the blockouts of this service, room and crew; with
    /// `provider`, of that provider.
    fn open(&self, service: Uuid, room: Uuid, crew: &[Uuid], provider: Option<Uuid>, span: Span) -> bool {
        let scheduled = self
            .schedules
            .iter()
            .filter(|(s, _)| {
                serves(&s.service_ids, service)
                    && s.resource_id.is_none_or(|r| r == room)
                    && s.provider_id.is_none_or(|p| provider.map_or(crew.contains(&p), |x| x == p))
            })
            .flat_map(|(_, open)| open.iter().copied());
        let blocked = self
            .blockouts
            .iter()
            .filter(|(b, _)| serves(&b.service_ids, service) && blocks(b, room, provider))
            .flat_map(|(_, closed)| closed.iter())
            .any(|closed| overlaps(*closed, span));
        covered(scheduled, span) && !blocked
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

/// One search: the request, its window and each party's rooms and crews.
struct Search<'a> {
    calendar: &'a Calendar,
    request: &'a Availability,
    window: Span,
    choices: Vec<(Vec<Uuid>, Vec<Vec<Uuid>>)>,
}

impl<'a> Search<'a> {
    fn new(calendar: &'a Calendar, request: &'a Availability, window: Span) -> Self {
        let choices = request.parties.iter().map(|party| (calendar.rooms(party), calendar.crews(party))).collect();
        Search { calendar, request, window, choices }
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
        let (rooms, crews) = &self.choices[placed.len()];
        let options: Vec<_> = self
            .start_options(party, first, &placed)
            .into_iter()
            .filter(|start| self.window.0 <= *start && *start + plan.client <= self.window.1)
            .cartesian_product(rooms.iter())
            .cartesian_product(crews.iter())
            .collect();
        let mut free_first = [false, true].into_iter().flat_map(|waitlist_allowed| options.iter().map(move |option| (waitlist_allowed, option)));
        free_first.find_map(|(waitlist_allowed, ((start, room), crew))| {
            let (waitlist, taken) = self.calendar.place(party, *start, **room, crew, &held, &[false, true])?;
            (waitlist_allowed || !waitlist).then_some(())?;
            let next = self.placed(party, *start, **room, crew, waitlist);
            self.extend(first, placed.iter().cloned().chain(once(next)).collect(), held.iter().cloned().chain(taken).collect())
        })
    }

    fn placed(&self, party: &Party, start: DateTime<Utc>, room: Uuid, crew: &[Uuid], waitlist: bool) -> Placed {
        let offset = self.request.from.timezone();
        Placed {
            client: party.client.clone(),
            service_id: party.service_id,
            providers: crew.iter().map(|id| Id { id: *id }).collect(),
            resources: vec![Id { id: room }],
            starts_at: start.with_timezone(&offset),
            ends_at: (start + self.calendar.plans[&party.service_id].client).with_timezone(&offset),
            waitlist,
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
    let calendar = Calendar::load(&mut conn, &request, window).await?;
    let slots: Vec<Slot> = Search::new(&calendar, &request, window).slots().collect();
    let next_available_day = match slots.is_empty() {
        true => next_available_day(&mut conn, &request, window.1).await?,
        false => None,
    };
    Ok(Slots { slots, next_available_day })
}

async fn next_available_day(conn: &mut PgConnection, request: &Availability, after: DateTime<Utc>) -> ApiResult<Option<NaiveDate>> {
    let window = (after, after + LOOK_AHEAD);
    let calendar = Calendar::load(conn, request, window).await?;
    let first = Search::new(&calendar, request, window).slots().next();
    Ok(first.map(|slot| slot.parties[0].starts_at.date_naive()))
}

/// The time `appointment` takes from each participant, read in the caller's transaction, if it fits as its
/// status asks: `booked` needs a free place, `waitlist` a waitlist place.
pub(super) async fn fit(conn: &mut PgConnection, appointment: &AppointmentData) -> ApiResult<Vec<(Uuid, Span)>> {
    let party = Party {
        client: Client::Known { id: appointment.client_id },
        service_id: appointment.service_id,
        providers: Vec::new(),
        resources: Vec::new(),
    };
    let request = Availability {
        org_id: appointment.org_id,
        from: appointment.starts_at.fixed_offset(),
        to: appointment.ends_at.fixed_offset(),
        parties: vec![party],
        service_gap: Duration::default(),
        client_gap: Duration::default(),
    };
    let calendar = Calendar::load(conn, &request, (appointment.starts_at, appointment.ends_at)).await?;
    let bad = |reason: &str| ApiError::BadRequest(reason.to_string());
    let length = calendar.plans[&appointment.service_id].client;
    (appointment.starts_at + length == appointment.ends_at).then_some(()).ok_or_else(|| bad("ends_at must be starts_at plus the service's time"))?;
    let crew = calendar.crew(appointment.service_id, &appointment.provider_ids).ok_or_else(|| bad("provider_ids must be one provider per role of the service"))?;
    let waitlist = appointment.status == Status::Waitlist;
    let (_, taken) = calendar
        .place(&request.parties[0], appointment.starts_at, appointment.resource_id, &crew, &[], &[waitlist])
        .ok_or_else(|| ApiError::Conflict(format!("the time is not open for a {:?} appointment", appointment.status)))?;
    Ok(taken.into_iter().filter_map(|(who, taken)| match who {
        Who::Id(id) => Some((id, taken.span)),
        Who::Tag(_) => None,
    }).collect())
}

async fn rows<T: Row>(conn: &mut PgConnection, org_id: Uuid) -> ApiResult<Vec<Entity<Uuid, T>>> {
    let query = format!("SELECT * FROM {} WHERE org_id = $1 AND deleted_at IS NULL ORDER BY id", T::TABLE_NAME);
    Ok(sqlx::query_as(&query).bind(org_id).fetch_all(conn).await?)
}

/// The time stored appointments take, by participant, overlapping `horizon`.
async fn taken(conn: &mut PgConnection, org_id: Uuid, horizon: Span) -> ApiResult<HashMap<Uuid, Vec<Taken>>> {
    let rows: Vec<(Uuid, PgRange<DateTime<Utc>>, Status)> = sqlx::query_as(
        "SELECT o.participant_id, o.during, a.status FROM occupancies o \
         JOIN appointments a ON a.org_id = o.org_id AND a.id = o.appointment_id \
         WHERE o.org_id = $1 AND o.during && $2 AND a.deleted_at IS NULL \
         AND a.status IN ('waitlist', 'booked', 'confirmed')",
    )
    .bind(org_id)
    .bind(PgRange::from(horizon.0..horizon.1))
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(who, during, status)| span(&during).map(|span| (who, Taken { span, waitlist: status == Status::Waitlist })))
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

/// Whether the windows together cover `span` without a gap.
fn covered(windows: impl Iterator<Item = Span>, span: Span) -> bool {
    let reach = windows
        .sorted_by_key(|window| window.0)
        .fold(span.0, |reach, window| if window.0 <= reach && reach < window.1 { window.1 } else { reach });
    reach >= span.1
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
