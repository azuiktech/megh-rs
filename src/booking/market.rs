//! What a search knows: the bookables, the time already taken and the open hours, and how a service is placed on them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use itertools::Itertools;
use sqlx::PgPool;

use super::rules::Rules;
use super::times::{covers, overlap, Times};
use super::variants::resolve;
use super::{
    Allocation, Attributes, Axes, BookableId, BookableRow, ClientId, ClientRow, Condition, Engagement, Error, Grouped, Groups, Interval, Need, OccupancyRow, OrgId,
    Party, Result, SearchRequest, Service, ServiceId, ServiceRow, ServiceWant,
};

/// The bookables picked for each role of a service.
pub(super) type Picks = Vec<Vec<usize>>;

/// One kind of bookable a service needs, shared by every step that asks for the same thing.
struct Role {
    candidates: Vec<usize>,
    count: usize,
}

/// A service wanted by a party, with every way its roles can be filled.
pub(super) struct Plan {
    pub(super) service: Arc<ServiceRow>,
    pub(super) seats: u32,
    client: Option<Attributes>,
    need_roles: Vec<Vec<usize>>,
    pub(super) assignments: Vec<Picks>,
}

pub(super) struct Placed {
    pub(super) allocations: Vec<Allocation>,
    pub(super) price: i64,
    pub(super) end: DateTime<Utc>,
}

pub(super) struct Market {
    bookables: Vec<BookableRow>,
    services: HashMap<ServiceId, Arc<ServiceRow>>,
    clients: HashMap<ClientId, Attributes>,
    taken: Vec<OccupancyRow>,
    open: HashMap<(BookableId, ServiceId), Times>,
    within: Interval,
}

pub(super) fn span(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or_else(|_| chrono::Duration::zero())
}

fn role_key(who: &Condition) -> String {
    serde_json::to_string(who).unwrap_or_default()
}

/// The roles of a service (needs with the same condition are one role) and which role each need is.
fn roles(service: &Service, bookables: &[BookableRow]) -> (Vec<Role>, Vec<Vec<usize>>) {
    let mut keys: Vec<String> = vec![];
    let mut roles: Vec<Role> = vec![];
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
                        roles.push(Role { candidates, count: 0 });
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

fn assignments(roles: &[Role]) -> Vec<Picks> {
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
    pub(super) async fn load(pool: &PgPool, request: &SearchRequest) -> Result<Self> {
        let all: Vec<BookableRow> = sqlx::query_as("SELECT * FROM bookables WHERE deleted_at IS NULL ORDER BY created_at, id").fetch_all(pool).await?;
        let orgs = Groups::load(pool, Grouped::Org).await?;
        let scoped: Vec<_> = all.into_iter().map(|bookable| Ok((in_scope(&orgs, request.org, &bookable)?, bookable))).collect::<Result<_>>()?;
        let bookables: Vec<BookableRow> = scoped.into_iter().filter(|(inside, _)| *inside).map(|(_, bookable)| bookable).collect();
        let taken = Self::taken(pool, &request.within).await?;
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
        Ok(Self { bookables, services, clients, taken, open, within: request.within })
    }

    /// Occupancies of live appointments and of holds that have not expired.
    async fn taken(pool: &PgPool, within: &Interval) -> Result<Vec<OccupancyRow>> {
        let query = "SELECT o.* FROM occupancies o LEFT JOIN holds h ON h.id = o.hold_id
            WHERE o.deleted_at IS NULL AND o.starts_at < $2 AND o.ends_at > $1 AND (o.appointment_id IS NOT NULL OR (h.deleted_at IS NULL AND h.expires_at > now()))";
        Ok(sqlx::query_as(query).bind(within.start).bind(within.end).fetch_all(pool).await?)
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
                Ok(self.plan(want, party.seats, Arc::clone(service), client.clone()))
            })
            .collect()
    }

    fn plan(&self, want: &ServiceWant, seats: u32, service: Arc<ServiceRow>, client: Option<Attributes>) -> Plan {
        let (roles, need_roles) = roles(&service.data, &self.bookables);
        let wanted = |picks: &Picks| {
            let chosen = picks.iter().flatten().map(|i| &self.bookables[*i].attributes);
            want.wants.iter().all(|filter| chosen.clone().any(|attributes| filter.holds(attributes)))
        };
        let assignments = assignments(&roles).into_iter().filter(wanted).collect();
        Plan { service, seats, client, need_roles, assignments }
    }

    fn in_use(&self, bookable: &BookableRow, during: &Interval, placed: &[Allocation]) -> u32 {
        let existing = self.taken.iter().filter(|row| row.bookable_id == bookable.id && row.engagement == Engagement::Engaged);
        let existing = existing.filter(|row| overlap(&Interval { start: row.starts_at, end: row.ends_at }, during)).map(|row| row.seats);
        let ours = placed.iter().filter(|a| a.bookable == bookable.id && a.engagement == Engagement::Engaged && overlap(&a.during, during));
        existing.chain(ours.map(|a| a.seats)).sum()
    }

    fn fits(&self, bookable: &BookableRow, service: ServiceId, allocation: &Allocation, placed: &[Allocation]) -> bool {
        let open = self.open.get(&(bookable.id, service));
        open.is_some_and(|open| covers(open, &allocation.during)) && self.in_use(bookable, &allocation.during, placed) + allocation.seats <= bookable.capacity
    }

    fn allocate(&self, plan: &Plan, need: &Need, pick: usize, during: Interval, placed: &[Allocation]) -> Option<Allocation> {
        let bookable = &self.bookables[pick];
        let allocation = Allocation { bookable: bookable.id, during, engagement: need.engagement, seats: plan.seats };
        let blocking = need.engagement == Engagement::Engaged;
        (!blocking || self.fits(bookable, plan.service.id, &allocation, placed)).then_some(allocation)
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
        let (end, allocations) = durations.iter().enumerate().try_fold((start, vec![]), |(cursor, mut mine), (index, duration)| {
            let during = Interval { start: cursor, end: cursor + span(*duration) };
            mine.extend(self.step(plan, picks, index, during, &[before, &mine].concat())?);
            Some((during.end, mine))
        })?;
        let inside = self.within.start <= start && end <= self.within.end;
        inside.then_some(Placed { allocations, price, end })
    }
}
