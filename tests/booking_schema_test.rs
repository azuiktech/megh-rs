//! Every booking table maps to its type through `Table`, and cancelling an appointment releases its time.

use chrono::{Duration, Utc};
use megh::booking::*;
use megh::TableEntity;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
struct Always;

#[typetag::serde]
impl Predicate for Always {
    fn holds(&self, _: &Attributes) -> bool {
        true
    }
}

fn attributes() -> Attributes {
    serde_json::json!({"k": 1})
}

fn seconds(count: u64) -> std::time::Duration {
    std::time::Duration::from_secs(count)
}

async fn org(pool: &PgPool) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'o')").bind(id).execute(pool).await.unwrap();
    id
}

#[sqlx::test(migrations = false)]
async fn bookables_clients_and_services_round_trip(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let bookable = Bookable { org_id: org, name: "r".into(), capacity: 2, waitlist: 0, attributes: attributes() };
    let bookable = BookableRow::new(BookableId(Uuid::new_v4()), bookable).insert(&pool).await.unwrap();
    assert_eq!((bookable.capacity, bookable.attributes.clone()), (2, attributes()));
    ClientRow::new(ClientId(Uuid::new_v4()), Client { org_id: org, name: "c".into(), first_name: "A".into(), last_name: "B".into(), email: None, home_phone: None, work_phone: None, mobile_phone: None, attributes: attributes() }).insert(&pool).await.unwrap();
    let need = Need { who: Box::new(Always), count: 1, engagement: Engagement::Free, role: Role::Provider };
    let step = Step { duration: seconds(60), client: Presence::Present, needs: vec![need], attributes: attributes() };
    let price = Price { micros: 5, currency: "USD".into() };
    let service = Service { org_id: org, name: "s".into(), mode: Mode::Class, price, steps: vec![step], variants: vec![], attributes: attributes() };
    let service = ServiceRow::new(ServiceId(Uuid::new_v4()), service).insert(&pool).await.unwrap();
    assert_eq!(service.mode, Mode::Class);
    assert_eq!(service.steps[0].needs[0].engagement, Engagement::Free);
}

#[sqlx::test(migrations = false)]
async fn schedules_and_groupings_round_trip(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    GroupingRow::new(GroupingId(Uuid::new_v4()), Grouping { org_id: org, kind: Grouped::Org, member_id: org, group_id: Uuid::new_v4() }).insert(&pool).await.unwrap();
    let schedule = Schedule {
        org_id: org,
        service_id: None,
        bookable_id: None,
        kind: Kind::Blockout,
        recurrence: "DTSTART:20261104T120000Z\nRRULE:FREQ=DAILY".parse().unwrap(),
        span: Span(seconds(3600)),
        reason: Some("lunch".into()),
        notes: None,
    };
    let schedule = ScheduleRow::new(ScheduleId(Uuid::new_v4()), schedule).insert(&pool).await.unwrap();
    assert_eq!((schedule.kind, schedule.span, schedule.reason.as_deref()), (Kind::Blockout, Span(seconds(3600)), Some("lunch")));
}

#[sqlx::test(migrations = false)]
async fn appointments_holds_and_the_rest_round_trip(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let now = Utc::now();
    let client = Client { org_id: org, name: "c".into(), first_name: "A".into(), last_name: "B".into(), email: None, home_phone: None, work_phone: None, mobile_phone: None, attributes: attributes() };
    let client = ClientRow::new(ClientId(Uuid::new_v4()), client).insert(&pool).await.unwrap();
    let method = PaymentMethod {
        org_id: org,
        name: "pass".into(),
        owner_id: Some(client.id),
        eligible: Box::new(Always),
        settlement: Settlement::Prepaid { balance: Amount::Uses(30), valid: None },
        attributes: attributes(),
    };
    PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), method).insert(&pool).await.unwrap();
    let held = held_appointment(&pool, org, now).await;
    assert_eq!((held.status, held.provider_ids.len(), held.payment.clone()), (Status::Held, 0, None));
}

#[sqlx::test(migrations = false)]
async fn cancelling_an_appointment_releases_its_time(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let bookable = Bookable { org_id: org, name: "r".into(), capacity: 1, waitlist: 0, attributes: attributes() };
    let bookable = BookableRow::new(BookableId(Uuid::new_v4()), bookable).insert(&pool).await.unwrap();
    let client = Client { org_id: org, name: "c".into(), first_name: "A".into(), last_name: "B".into(), email: None, home_phone: None, work_phone: None, mobile_phone: None, attributes: attributes() };
    let client = ClientRow::new(ClientId(Uuid::new_v4()), client).insert(&pool).await.unwrap();
    let service = service_of(&pool, org).await;
    let now = Utc::now();
    let appointment = Appointment {
        org_id: org,
        client_id: client.id,
        service_id: service.id,
        provider_ids: vec![],
        room_id: bookable.id,
        starts_at: now,
        ends_at: now + Duration::hours(1),
        status: Status::Booked,
        expires_at: None,
        payment: None,
        attributes: attributes(),
    };
    let appointment = AppointmentRow::new(AppointmentId(Uuid::new_v4()), appointment).insert(&pool).await.unwrap();
    let occupancy = Occupancy {
        appointment_id: appointment.id,
        bookable_id: Some(bookable.id),
        client_id: None,
        starts_at: now,
        ends_at: now + Duration::hours(1),
        engagement: Engagement::OnDemand,
        status: Status::Booked,
    };
    let occupancy = OccupancyRow::new(OccupancyId(Uuid::new_v4()), occupancy).insert(&pool).await.unwrap();
    assert_eq!(occupancy.engagement, Engagement::OnDemand);
    appointment.soft_delete(&pool).await.unwrap().unwrap();
    let blocking: i64 = sqlx::query_scalar("SELECT count(*) FROM occupancies WHERE deleted_at IS NULL").fetch_one(&pool).await.unwrap();
    assert_eq!(blocking, 0);
}

async fn service_of(pool: &PgPool, org: Uuid) -> ServiceRow {
    let price = Price { micros: 5, currency: "USD".into() };
    let service = Service { org_id: org, name: "s".into(), mode: Mode::Appointment, price, steps: vec![], variants: vec![], attributes: attributes() };
    ServiceRow::new(ServiceId(Uuid::new_v4()), service).insert(pool).await.unwrap()
}

async fn held_appointment(pool: &PgPool, org: Uuid, now: chrono::DateTime<Utc>) -> AppointmentRow {
    let room = BookableRow::new(BookableId(Uuid::new_v4()), Bookable { org_id: org, name: "h".into(), capacity: 1, waitlist: 0, attributes: attributes() }).insert(pool).await.unwrap();
    let client = ClientRow::new(ClientId(Uuid::new_v4()), Client { org_id: org, name: "h".into(), first_name: "A".into(), last_name: "B".into(), email: None, home_phone: None, work_phone: None, mobile_phone: None, attributes: attributes() }).insert(pool).await.unwrap();
    let service = service_of(pool, org).await;
    let hold = Appointment {
        org_id: org,
        client_id: client.id,
        service_id: service.id,
        provider_ids: vec![],
        room_id: room.id,
        starts_at: now,
        ends_at: now + Duration::minutes(30),
        status: Status::Held,
        expires_at: Some(now + Duration::minutes(5)),
        payment: None,
        attributes: attributes(),
    };
    AppointmentRow::new(AppointmentId(Uuid::new_v4()), hold).insert(pool).await.unwrap()
}
