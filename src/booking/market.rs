//! What a search knows: the bookables, the time already taken and the open hours, and how a service is placed on them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use itertools::Itertools;
use sqlx::PgPool;
use uuid::Uuid;

use super::rules::Rules;
use super::busy::busy;
use super::times::{clash, covers, overlap, Times};
use super::variants::resolve;
use super::{
    Allocation, Attributes, Axes, BookableId, BookableRow, ClientId, ClientRow, Condition, Engagement, Error, Grouped, Groups, Interval, Need, OccupancyRow, OrgId,
    Party, Presence, Result, SearchRequest, Service, ServiceId, ServiceRow, ServiceWant, Status,
};

/// The bookables picked for each role of a service.
pub(super) type Picks = Vec<Vec<usize>>;

/// One kind of bookable a service needs, shared by every step that asks for the same thing.
struct Pool {
    candidates: Vec<usize>,
    count: usize,
}

/// A service wanted by a party, with every way its roles can be filled.
pub(super) struct Plan {
    pub(super) service: Arc<ServiceRow>,
    client: Option<Attributes>,
    client_id: Option<ClientId>,
    need_roles: Vec<Vec<usize>>,
    pub(super) assignments: Vec<Picks>,
}

pub(super) struct Placed {
    pub(super) allocations: Vec<Allocation>,
    pub(super) price: i64,
    pub(super) end: DateTime<Utc>,
    /// When the client is there: from the first step they are present to the last.
    pub(super) client: Interval,
}

pub(super) struct Market {
    bookables: Vec<BookableRow>,
    services: HashMap<ServiceId, Arc<ServiceRow>>,
    clients: HashMap<ClientId, Attributes>,
    taken: Vec<OccupancyRow>,
    open: HashMap<(BookableId, ServiceId), Times>,
    /// When the clients of the search are already booked or held.
    busy: HashMap<ClientId, Times>,
    within: Interval,
}

pub(super) fn span(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or_else(|_| chrono::Duration::zero())
}

fn role_key(who: &Condition) -> String {
    serde_json::to_string(who).unwrap_or_default()
}

/// The roles of a service (needs with the same condition are one role) and which role each need is.
fn roles(service: &Service, bookables: &[BookableRow]) -> (Vec<Pool>, Vec<Vec<usize>>) {
    let mut keys: Vec<String> = vec![];
    let mut roles: Vec<Pool> = vec![];
    let need_roles = service
        .steps
        .iter()
        .map(|step| {
            step.needs
                .iter()
                .map(|need| {
                    let key = role_key(&need.who);
                    let index = keys.iter().position(|known| *known == key).unwrap_or_else(|| {
                        keys.push(key);
                        let candidates = (0..bookables.len()).filter(|i| need.who.holds(&bookables[*i].attributes)).collect();
                        roles.push(Pool { candidates, count: 0 });
                        roles.len() - 1
                    });
                    roles[index].count = roles[index].count.max(need.count as usize);
                    index
                })
                .collect()
        })
        .collect();
    (roles, need_roles)
}

fn assignments(roles: &[Pool]) -> Vec<Picks> {
    match roles.is_empty() {
        true => vec![vec![]],
        false => roles.iter().map(|role| role.candidates.iter().copied().combinations(role.count).collect::<Vec<_>>()).multi_cartesian_product().collect(),
    }
}

fn in_scope(orgs: &Groups, root: Option<OrgId>, bookable: &BookableRow) -> Result<bool> {
    match root {
        None => Ok(true),
        Some(root) => Ok(orgs.reach(bookable.org_id)?.contains_key(&root)),
    }
}

impl Market {
    /// `ignoring` is an appointment or hold whose own time counts for nothing: the one being moved.
    pub(super) async fn load(pool: &PgPool, request: &SearchRequest, ignoring: Option<Uuid>) -> Result<Self> {
        let all: Vec<BookableRow> = sqlx::query_as("SELECT * FROM bookables WHERE deleted_at IS NULL ORDER BY created_at, id").fetch_all(pool).await?;
        let orgs = Groups::load(pool, Grouped::Org).await?;
        let scoped: Vec<_> = all.into_iter().map(|bookable| Ok((in_scope(&orgs, request.org, &bookable)?, bookable))).collect::<Result<_>>()?;
        let bookables: Vec<BookableRow> = scoped.into_iter().filter(|(inside, _)| *inside).map(|(_, bookable)| bookable).collect();
        let taken = Self::taken(pool, &request.within, ignoring).await?;
        let rules = Rules::load(pool).await?;
        let services: Vec<ServiceId> = request.parties.iter().flat_map(|party| party.services.iter().map(|want| want.service)).unique().collect();
        let open = bookables
            .iter()
            .cartesian_product(services)
            .map(|(bookable, service)| {
                let axes = Axes { org: bookable.org_id, service, bookable: bookable.id };
                Ok(((bookable.id, service), rules.times(&axes, &request.within)?))
            })
            .collect::<Result<_>>()?;
        let (services, clients) = (Self::services(pool, request).await?, Self::clients(pool, request).await?);
        let named: Vec<ClientId> = request.parties.iter().filter_map(|party| party.client).collect();
        Ok(Self { bookables, services, clients, taken, open, busy: busy(pool, &named, &request.within, ignoring).await?, within: request.within })
    }

    /// Occupancies of live appointments, among them holds that have not expired.
    async fn taken(pool: &PgPool, within: &Interval, ignoring: Option<Uuid>) -> Result<Vec<OccupancyRow>> {
        let query = "SELECT o.* FROM occupancies o JOIN appointments a ON a.id = o.appointment_id
            WHERE o.bookable_id IS NOT NULL AND o.deleted_at IS NULL AND o.starts_at < $2 AND o.ends_at > $1
            AND a.deleted_at IS NULL AND (a.status <> 'held' OR a.expires_at > now()) AND ($3::uuid IS NULL OR o.appointment_id <> $3)";
        Ok(sqlx::query_as(query).bind(within.start).bind(within.end).bind(ignoring).fetch_all(pool).await?)
    }

    async fn services(pool: &PgPool, request: &SearchRequest) -> Result<HashMap<ServiceId, Arc<ServiceRow>>> {
        let ids: Vec<ServiceId> = request.parties.iter().flat_map(|party| party.services.iter().map(|want| want.service)).unique().collect();
        let rows: Vec<ServiceRow> = sqlx::query_as("SELECT * FROM services WHERE id = ANY($1) AND deleted_at IS NULL").bind(ids).fetch_all(pool).await?;
        Ok(rows.into_iter().map(|row| (row.id, Arc::new(row))).collect())
    }

    async fn clients(pool: &PgPool, request: &SearchRequest) -> Result<HashMap<ClientId, Attributes>> {
        let ids: Vec<ClientId> = request.parties.iter().filter_map(|party| party.client).unique().collect();
        let rows: Vec<ClientRow> = sqlx::query_as("SELECT * FROM clients WHERE id = ANY($1) AND deleted_at IS NULL").bind(ids).fetch_all(pool).await?;
        Ok(rows.into_iter().map(|row| (row.id, row.data.attributes)).collect())
    }

    /// The plans of a party's services, in order.
    pub(super) fn plans(&self, party: &Party) -> Result<Vec<Plan>> {
        let client = party.client.map(|id| self.clients.get(&id).cloned().ok_or(Error::NotFound)).transpose()?;
        party
            .services
            .iter()
            .map(|want| {
                let service = self.services.get(&want.service).ok_or(Error::NotFound)?;
                Ok(self.plan(want, Arc::clone(service), (client.clone(), party.client)))
            })
            .collect()
    }

    fn plan(&self, want: &ServiceWant, service: Arc<ServiceRow>, (client, client_id): (Option<Attributes>, Option<ClientId>)) -> Plan {
        let (roles, need_roles) = roles(&service.data, &self.bookables);
        let wanted = |picks: &Picks| {
            let chosen = picks.iter().flatten().map(|i| &self.bookables[*i].attributes);
            want.wants.iter().all(|filter| chosen.clone().any(|attributes| filter.holds(attributes)))
        };
        let assignments = assignments(&roles).into_iter().filter(wanted).collect();
        Plan { service, client, client_id, need_roles, assignments }
    }

    /// The seats of the bookable used in the time by engaged rows with one of these statuses, already taken or placed.
    fn in_use(&self, bookable: &BookableRow, during: &Interval, placed: &[Allocation], statuses: &[Status]) -> u32 {
        let existing = self.taken.iter().filter(|row| row.bookable_id == Some(bookable.id) && row.engagement == Engagement::Engaged && statuses.contains(&row.status));
        let existing = existing.filter(|row| overlap(&Interval { start: row.starts_at, end: row.ends_at }, during)).map(|_| 1);
        let ours = placed.iter().filter(|a| a.bookable == bookable.id && a.engagement == Engagement::Engaged && statuses.contains(&a.status) && overlap(&a.during, during));
        existing.chain(ours.map(|_| 1)).sum()
    }

    /// `booked` while the room has capacity, `waitlist` while it has a place, otherwise `None`; `None` too when closed.
    fn status_in(&self, bookable: &BookableRow, service: ServiceId, allocation: &Allocation, placed: &[Allocation]) -> Option<Status> {
        let open = self.open.get(&(bookable.id, service)).is_some_and(|open| covers(open, &allocation.during));
        let used = |statuses: &[Status]| self.in_use(bookable, &allocation.during, placed, statuses);
        let seats = 1;
        if !open {
            None
        } else if used(&[Status::Held, Status::Booked, Status::Confirmed]) + seats <= bookable.capacity {
            Some(Status::Booked)
        } else {
            (used(&[Status::Waitlist]) + seats <= bookable.waitlist).then_some(Status::Waitlist)
        }
    }

    fn allocate(&self, plan: &Plan, need: &Need, pick: usize, during: Interval, placed: &[Allocation]) -> Option<Allocation> {
        let bookable = &self.bookables[pick];
        let allocation = Allocation { bookable: bookable.id, during, engagement: need.engagement, role: need.role, placement: 0, status: Status::Booked };
        match need.engagement {
            Engagement::Engaged => self.status_in(bookable, plan.service.id, &allocation, placed).map(|status| Allocation { status, ..allocation }),
            _ => Some(allocation),
        }
    }

    /// The allocations of one step, or `None` when a need cannot be met.
    fn step(&self, plan: &Plan, picks: &Picks, index: usize, during: Interval, placed: &[Allocation]) -> Option<Vec<Allocation>> {
        let mut needs = plan.service.steps[index].needs.iter().zip(&plan.need_roles[index]);
        needs.try_fold(vec![], |made, (need, role)| {
            picks[*role].iter().take(need.count as usize).try_fold(made, |mut made, pick| {
                made.push(self.allocate(plan, need, *pick, during, &[placed, &made].concat())?);
                Some(made)
            })
        })
    }

    /// The service started at `start` with these picks, or `None` when it does not fit.
    pub(super) fn place(&self, plan: &Plan, picks: &Picks, start: DateTime<Utc>, before: &[Allocation]) -> Option<Placed> {
        let people = picks.iter().flatten().map(|i| &self.bookables[*i].attributes).chain(plan.client.iter());
        let (durations, price) = resolve(&plan.service, &people.collect::<Vec<_>>());
        let (end, allocations, present) = durations.iter().enumerate().try_fold((start, vec![], vec![]), |(cursor, mut mine, mut present), (index, duration)| {
            let during = Interval { start: cursor, end: cursor + span(*duration) };
            mine.extend(self.step(plan, picks, index, during, &[before, &mine].concat())?);
            if plan.service.steps[index].client == Presence::Present {
                present.push(during);
            }
            Some((during.end, mine, present))
        })?;
        let client = Interval { start: present.iter().map(|during| during.start).min().unwrap_or(start), end: present.iter().map(|during| during.end).max().unwrap_or(end) };
        let inside = self.within.start <= start && end <= self.within.end;
        let free = !plan.client_id.and_then(|client_id| self.busy.get(&client_id)).is_some_and(|calendar| clash(calendar, &client));
        (inside && free).then_some(Placed { allocations, price, end, client })
    }
}
