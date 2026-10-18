//! Specification: docs/TPD-booking.md, section 10 (eligibility is the schedule).
//! Behaviour on a database with nothing booked. Seeded scenarios are in the other booking tests.

use chrono::{Duration, Utc};
use megh::booking::{Appointments, Availability, Axes, BookableId, Calendar, ClientId, Confirmation, Error, HoldId, IdempotencyKey, Interval, Queue, Reservations, ServiceId, Target, Waitlist};
use megh::booking::AppointmentId;
use sqlx::PgPool;
use uuid::Uuid;

fn day() -> Interval {
    let start = Utc::now();
    Interval { start, end: start + Duration::days(1) }
}

#[sqlx::test]
async fn a_combination_with_no_open_rule_has_no_open_time(pool: PgPool) {
    let axes = Axes { org: Uuid::new_v4(), service: ServiceId(Uuid::new_v4()), bookable: BookableId(Uuid::new_v4()) };
    assert_eq!(Calendar::new(pool).open(&axes, &day()).await.unwrap(), vec![]);
}

fn confirmation() -> Confirmation {
    Confirmation { clients: vec![], payer: ClientId(Uuid::new_v4()), payments: vec![], attributes: Default::default() }
}

#[sqlx::test]
async fn cancelling_an_unknown_appointment_is_not_found(pool: PgPool) {
    let result = Reservations::new(pool).cancel(AppointmentId(Uuid::new_v4())).await;
    assert!(matches!(result, Err(Error::NotFound)));
}

#[sqlx::test]
async fn confirming_an_unknown_hold_is_not_found(pool: PgPool) {
    let key = IdempotencyKey("key".into());
    let result = Reservations::new(pool).confirm(HoldId(Uuid::new_v4()), confirmation(), key).await;
    assert!(matches!(result, Err(Error::NotFound)));
}

#[sqlx::test]
async fn nobody_waits_for_an_unknown_target(pool: PgPool) {
    let target = Target::Window(ServiceId(Uuid::new_v4()), day());
    assert_eq!(Queue::new(pool).waiting(&target).await.unwrap(), vec![]);
}
