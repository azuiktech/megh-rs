//! Specification: docs/TPD-booking.md, section 10 (eligibility is the schedule).
//! Behaviour on a database with nothing booked. Seeded scenarios are in the other booking tests.

use chrono::{Duration, Utc};
use megh::booking::{Amount, Appointments, Availability, Axes, BookableId, Calendar, ClientId, Error, IdempotencyKey, Interval, Payment, PaymentMethodId, Reservations, ServiceId};
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

fn payment() -> Payment {
    Payment { payer_id: ClientId(Uuid::new_v4()), method: PaymentMethodId(Uuid::new_v4()), amount: Amount::Uses(1) }
}

#[sqlx::test]
async fn cancelling_an_unknown_appointment_is_not_found(pool: PgPool) {
    let result = Reservations::new(pool).cancel(AppointmentId(Uuid::new_v4())).await;
    assert!(matches!(result, Err(Error::NotFound)));
}

#[sqlx::test]
async fn confirming_an_unknown_hold_is_not_found(pool: PgPool) {
    let key = IdempotencyKey("key".into());
    let result = Reservations::new(pool).book_held(AppointmentId(Uuid::new_v4()), payment(), key).await;
    assert!(matches!(result, Err(Error::NotFound)));
}

