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
    Attributes::from([("k".to_string(), serde_json::json!(1))])
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
    let bookable = Bookable { org_id: org, name: "r".into(), capacity: 2, attributes: attributes() };
    let bookable = BookableRow::new(BookableId(Uuid::new_v4()), bookable).insert(&pool).await.unwrap();
    assert_eq!((bookable.capacity, bookable.attributes.clone()), (2, attributes()));
    ClientRow::new(ClientId(Uuid::new_v4()), Client { org_id: Some(org), name: "c".into(), attributes: attributes() }).insert(&pool).await.unwrap();
    let need = Need { who: Box::new(Always), count: 1, engagement: Engagement::Free };
    let step = Step { duration: seconds(60), client: Presence::Present, needs: vec![need], attributes: attributes() };
    let price = Price { micros: 5, currency: "USD".into() };
    let service = Service { org_id: Some(org), name: "s".into(), mode: Mode::Class, price, steps: vec![step], variants: vec![], attributes: attributes() };
    let service = ServiceRow::new(ServiceId(Uuid::new_v4()), service).insert(&pool).await.unwrap();
    assert_eq!(service.mode, Mode::Class);
    assert_eq!(service.steps[0].needs[0].engagement, Engagement::Free);
}

#[sqlx::test(migrations = false)]
async fn schedules_and_groupings_round_trip(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    GroupingRow::new(Uuid::new_v4(), Grouping { kind: Grouped::Org, member_id: org, group_id: Uuid::new_v4() }).insert(&pool).await.unwrap();
    let schedule = Schedule {
        org_id: Some(org),
        service_id: None,
        bookable_id: None,
        kind: Kind::Blockout,
        recurrence: Recurrence("DTSTART:20261104T120000Z\nRRULE:FREQ=DAILY".into()),
        span: seconds(3600),
        reason: Some("lunch".into()),
        notes: None,
    };
    let schedule = ScheduleRow::new(ScheduleId(Uuid::new_v4()), schedule).insert(&pool).await.unwrap();
    assert_eq!((schedule.kind, schedule.span, schedule.reason.as_deref()), (Kind::Blockout, seconds(3600), Some("lunch")));
}

#[sqlx::test(migrations = false)]
async fn appointments_holds_and_the_rest_round_trip(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let now = Utc::now();
    let client = Client { org_id: Some(org), name: "c".into(), attributes: attributes() };
    let client = ClientRow::new(ClientId(Uuid::new_v4()), client).insert(&pool).await.unwrap();
    let method = PaymentMethod {
        org_id: Some(org),
        name: "pass".into(),
        owner_id: Some(client.id),
        eligible: Box::new(Always),
        settlement: Settlement::Prepaid { balance: Amount::Uses(30), valid: None },
        attributes: attributes(),
    };
    PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), method).insert(&pool).await.unwrap();
    let hold = Hold { org_id: Some(org), tz: "UTC".into(), parties: vec![], expires_at: now + Duration::minutes(5), attributes: attributes() };
    HoldRow::new(HoldId(Uuid::new_v4()), hold).insert(&pool).await.unwrap();
    let target = Target::Window(ServiceId(Uuid::new_v4()), Interval { start: now, end: now + Duration::days(1) });
    let entry = WaitlistEntry { client_id: client.id, target, requested_at: now, attributes: attributes() };
    WaitlistEntryRow::new(WaitlistEntryId(Uuid::new_v4()), entry).insert(&pool).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn cancelling_an_appointment_releases_its_time(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let bookable = Bookable { org_id: org, name: "r".into(), capacity: 1, attributes: attributes() };
    let bookable = BookableRow::new(BookableId(Uuid::new_v4()), bookable).insert(&pool).await.unwrap();
    let client = Client { org_id: Some(org), name: "c".into(), attributes: attributes() };
    let client = ClientRow::new(ClientId(Uuid::new_v4()), client).insert(&pool).await.unwrap();
    let appointment = Appointment {
        org_id: Some(org),
        tz: "UTC".into(),
        lines: vec![],
        payer_id: client.id,
        payments: vec![],
        idempotency_key: IdempotencyKey("k".into()),
        fingerprint: "f".into(),
        attributes: attributes(),
    };
    let appointment = AppointmentRow::new(AppointmentId(Uuid::new_v4()), appointment).insert(&pool).await.unwrap();
    let now = Utc::now();
    let occupancy = Occupancy {
        hold_id: None,
        appointment_id: Some(appointment.id),
        bookable_id: bookable.id,
        starts_at: now,
        ends_at: now + Duration::hours(1),
        engagement: Engagement::OnDemand,
        seats: 1,
    };
    let occupancy = OccupancyRow::new(OccupancyId(Uuid::new_v4()), occupancy).insert(&pool).await.unwrap();
    assert_eq!(occupancy.engagement, Engagement::OnDemand);
    appointment.soft_delete(&pool).await.unwrap().unwrap();
    let blocking: i64 = sqlx::query_scalar("SELECT count(*) FROM occupancies WHERE deleted_at IS NULL").fetch_one(&pool).await.unwrap();
    assert_eq!(blocking, 0);
}
