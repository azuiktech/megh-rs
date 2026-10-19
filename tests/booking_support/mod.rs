//! Fixtures for the booking specification tests: rows are seeded straight into the tables.
#![allow(dead_code)]

use std::time::Duration;

use chrono::{DateTime, Datelike, TimeZone, Utc, Weekday};
use megh::booking::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

pub mod rest;

#[derive(Debug, Serialize, Deserialize)]
pub struct Is {
    pub key: String,
    pub value: String,
}

#[typetag::serde]
impl Predicate for Is {
    fn holds(&self, subject: &Attributes) -> bool {
        subject.get(&self.key).and_then(|value| value.as_str()) == Some(self.value.as_str())
    }
}

pub fn is(key: &str, value: &str) -> Condition {
    Box::new(Is { key: key.into(), value: value.into() })
}

pub fn attributes(pairs: &[(&str, &str)]) -> Attributes {
    serde_json::Value::Object(pairs.iter().map(|(key, value)| (key.to_string(), serde_json::json!(value))).collect())
}

pub fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

/// Midnight UTC of the day `offset` days after a base day a week from now, so tests stay in the future.
pub fn day(offset: i64) -> DateTime<Utc> {
    let base = (Utc::now() + chrono::Duration::days(7 + offset)).date_naive();
    Utc.from_utc_datetime(&base.and_hms_opt(0, 0, 0).unwrap())
}

pub fn at(offset: i64, hour: u32, minute: u32) -> DateTime<Utc> {
    day(offset) + chrono::Duration::minutes(i64::from(hour * 60 + minute))
}

pub fn between(offset: i64, from_hour: u32, to_hour: u32) -> Interval {
    Interval { start: at(offset, from_hour, 0), end: at(offset, to_hour, 0) }
}

pub fn whole_day(offset: i64) -> Interval {
    Interval { start: day(offset), end: day(offset + 1) }
}

fn stamp(moment: DateTime<Utc>) -> String {
    moment.format("%Y%m%dT%H%M%SZ").to_string()
}

/// Every day from day 0 at `hour`.
pub fn daily(hour: u32) -> Recurrence {
    Recurrence(format!("DTSTART:{}\nRRULE:FREQ=DAILY", stamp(at(0, hour, 0))))
}

/// Only on the given day at `hour`.
pub fn once(offset: i64, hour: u32) -> Recurrence {
    Recurrence(format!("DTSTART:{}\nRRULE:FREQ=DAILY;COUNT=1", stamp(at(offset, hour, 0))))
}

pub fn weekly(days: &[Weekday], hour: u32) -> Recurrence {
    let names: Vec<_> = days.iter().map(|day| day.to_string().to_uppercase()[..2].to_string()).collect();
    Recurrence(format!("DTSTART:{}\nRRULE:FREQ=WEEKLY;BYDAY={}", stamp(at(0, hour, 0)), names.join(",")))
}

pub fn weekday(offset: i64) -> Weekday {
    day(offset).weekday()
}

pub async fn org(pool: &PgPool, name: &str) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, $2)").bind(id).bind(name).execute(pool).await.unwrap();
    id
}

pub async fn group(pool: &PgPool, kind: Grouped, member: Uuid, group: Uuid) {
    let owner = org(pool, "grouping").await;
    GroupingRow::new(GroupingId(Uuid::new_v4()), Grouping { org_id: owner, kind, member_id: member, group_id: group }).insert(pool).await.unwrap();
}

pub async fn bookable(pool: &PgPool, org: Uuid, name: &str, capacity: u32, tags: &[(&str, &str)]) -> BookableRow {
    let data = Bookable { org_id: org, name: name.into(), capacity, attributes: attributes(tags) };
    BookableRow::new(BookableId(Uuid::new_v4()), data).insert(pool).await.unwrap()
}

pub async fn client(pool: &PgPool) -> ClientRow {
    let owner = org(pool, "client").await;
    client_of(pool, owner).await
}

pub async fn client_of(pool: &PgPool, org: Uuid) -> ClientRow {
    let data = Client { org_id: org, name: "client".into(), first_name: "Cleo".into(), last_name: "Client".into(), email: None, home_phone: None, work_phone: None, mobile_phone: None, attributes: serde_json::json!({}) };
    ClientRow::new(ClientId(Uuid::new_v4()), data).insert(pool).await.unwrap()
}

pub fn price(micros: i64) -> Price {
    Price { micros, currency: "USD".into() }
}

fn step(duration: Duration, needs: Vec<Need>) -> Step {
    Step { duration, client: Presence::Present, needs, attributes: serde_json::json!({}) }
}

pub fn need(who: Condition, engagement: Engagement) -> Need {
    Need { who, count: 1, engagement }
}

pub async fn insert_service(pool: &PgPool, org: Uuid, name: &str, steps: Vec<Step>, variants: Vec<Variant>) -> ServiceRow {
    let data = Service { org_id: org, name: name.into(), mode: Mode::Appointment, price: price(100_000_000), steps, variants, attributes: serde_json::json!({}) };
    ServiceRow::new(ServiceId(Uuid::new_v4()), data).insert(pool).await.unwrap()
}

/// One step of `length` minutes engaging the provider (when given) and one room.
pub async fn service(pool: &PgPool, org: Uuid, name: &str, length: u64, provider: Option<Condition>, room: Condition) -> ServiceRow {
    let needs = provider.into_iter().map(|who| need(who, Engagement::Engaged)).chain([need(room, Engagement::Engaged)]).collect();
    insert_service(pool, org, name, vec![step(minutes(length), needs)], vec![]).await
}

#[derive(Default)]
pub struct Where {
    pub org: Option<Uuid>,
    pub service: Option<ServiceId>,
    pub bookable: Option<BookableId>,
}

/// The org that owns a rule: the one named, else that of its bookable or of its service.
async fn owner_of_rule(pool: &PgPool, axes: &Where) -> Uuid {
    match (axes.org, axes.bookable, axes.service) {
        (Some(org), ..) => org,
        (None, Some(bookable), _) => sqlx::query_scalar("SELECT org_id FROM bookables WHERE id = $1").bind(bookable).fetch_one(pool).await.unwrap(),
        (None, None, Some(service)) => sqlx::query_scalar("SELECT org_id FROM services WHERE id = $1").bind(service).fetch_one(pool).await.unwrap(),
        (None, None, None) => panic!("a rule belongs to an org"),
    }
}

pub async fn rule(pool: &PgPool, kind: Kind, axes: Where, recurrence: Recurrence, span: Duration) {
    let data = Schedule { org_id: owner_of_rule(pool, &axes).await, service_id: axes.service, bookable_id: axes.bookable, kind, recurrence, span, reason: None, notes: None };
    ScheduleRow::new(ScheduleId(Uuid::new_v4()), data).insert(pool).await.unwrap();
}

pub async fn open_rule(pool: &PgPool, axes: Where, from_hour: u32, hours: u64) {
    rule(pool, Kind::Open, axes, daily(from_hour), minutes(hours * 60)).await;
}

pub async fn blockout_rule(pool: &PgPool, axes: Where, recurrence: Recurrence, length: u64) {
    rule(pool, Kind::Blockout, axes, recurrence, minutes(length)).await;
}

async fn occupy(pool: &PgPool, hold: Option<HoldId>, booking: Option<AppointmentId>, bookable: BookableId, during: Interval, engagement: Engagement, seats: u32) {
    let data = Occupancy { hold_id: hold, appointment_id: booking, bookable_id: bookable, starts_at: during.start, ends_at: during.end, engagement, seats };
    OccupancyRow::new(OccupancyId(Uuid::new_v4()), data).insert(pool).await.unwrap();
}

/// An existing booking occupying `bookable`.
pub async fn booked(pool: &PgPool, bookable: BookableId, during: Interval, engagement: Engagement) -> AppointmentRow {
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM bookables WHERE id = $1").bind(bookable).fetch_one(pool).await.unwrap();
    let payer = client_of(pool, org).await;
    let data = Appointment { org_id: org, tz: "UTC".into(), lines: vec![], payer_id: payer.id, payments: vec![], idempotency_key: IdempotencyKey(Uuid::new_v4().to_string()), fingerprint: "seed".into(), attributes: serde_json::json!({}) };
    let booking = AppointmentRow::new(AppointmentId(Uuid::new_v4()), data).insert(pool).await.unwrap();
    occupy(pool, None, Some(booking.id), bookable, during, engagement, 1).await;
    booking
}

/// An existing hold on `bookable` that expires at `expires_at`.
pub async fn held(pool: &PgPool, bookable: BookableId, during: Interval, expires_at: DateTime<Utc>) -> HoldRow {
    let org: Uuid = sqlx::query_scalar("SELECT org_id FROM bookables WHERE id = $1").bind(bookable).fetch_one(pool).await.unwrap();
    let data = Hold { org_id: org, tz: "UTC".into(), parties: vec![], placements: vec![], expires_at, attributes: serde_json::json!({}) };
    let hold = HoldRow::new(HoldId(Uuid::new_v4()), data).insert(pool).await.unwrap();
    occupy(pool, Some(hold.id), None, bookable, during, Engagement::Engaged, 1).await;
    hold
}

pub fn request(service: ServiceId, within: Interval) -> SearchRequest {
    let party = Party { client: None, seats: 1, services: vec![ServiceWant { service, wants: vec![] }] };
    SearchRequest { org: None, parties: vec![party], within, party_offset: Duration::ZERO, service_gap: Duration::ZERO, slot_interval: minutes(15), limit: None }
}

pub fn allocation_of(slot: &Slot, bookable: BookableId) -> Vec<&Allocation> {
    slot.allocations.iter().filter(|allocation| allocation.bookable == bookable).collect()
}

pub fn overlaps(left: &Interval, right: &Interval) -> bool {
    left.start < right.end && right.start < left.end
}

pub fn length(allocation: &Allocation) -> Duration {
    (allocation.during.end - allocation.during.start).to_std().unwrap()
}
