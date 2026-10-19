//! Specification: docs/TPD-booking.md, section 13 and the user's answers of 2026-10-18: a predicate is checked before
//! the booking is paid (`eligible` over the service's `display_name`, `mode` and `attributes`), and a pass is judged
//! by the time of the appointment ("any days during the validity of the pass"), not the day it is used to book.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{
    Amount, Appointments, AppointmentRow, Confirmation, Error, Filter, IdempotencyKey, Interval, Payment, PaymentMethod, PaymentMethodId, PaymentMethodRow, Price, Reservations,
    Search, ServiceRow, Settlement, Slots,
};
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    pool: PgPool,
    org: Uuid,
    massage: ServiceRow,
    client: megh::booking::ClientRow,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "spa").await;
    for (name, tag) in [("suite", ("kind", "room")), ("tess", ("role", "therapist"))] {
        let found = bookable(&pool, org, name, 1, &[tag]).await;
        open_rule(&pool, Where { bookable: Some(found.id), ..Where::default() }, 9, 8).await;
    }
    let massage = service(&pool, org, "massage", 60, Some(is("role", "therapist")), is("kind", "room")).await;
    sqlx::query("UPDATE services SET attributes = '{\"category\": \"massage\"}' WHERE id = $1").bind(massage.id).execute(&pool).await.unwrap();
    let client = client(&pool).await;
    World { pool, org, massage, client }
}

impl World {
    async fn method(&self, eligible: &str, settlement: Settlement) -> PaymentMethodRow {
        let method = PaymentMethod { org_id: self.org, name: "method".into(), owner_id: None, eligible: Box::new(Filter::parse(eligible).unwrap()), settlement, attributes: serde_json::json!({}) };
        PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), method).insert(&self.pool).await.unwrap()
    }

    /// Books the hour of day 1 that starts at `hour` and pays with the method.
    async fn pay_with(&self, method: &PaymentMethodRow, amount: Amount, hour: u32) -> Result<AppointmentRow, Error> {
        let (request, key) = (request(self.massage.id, between(1, hour, hour + 1)), format!("hour-{hour}"));
        let slot = Slots::new(self.pool.clone()).find(&request).await.unwrap().remove(0);
        let reservations = Reservations::new(self.pool.clone());
        let hold = reservations.hold(&request, &slot, Utc::now() + Duration::minutes(10)).await.unwrap();
        let payments = vec![Payment { method: method.id, amount }];
        let confirmation = Confirmation { clients: vec![self.client.id], payer: self.client.id, payments, attributes: serde_json::json!({}) };
        reservations.confirm(hold.id, confirmation, IdempotencyKey(key)).await
    }

    async fn uses_left(&self, method: &PaymentMethodRow) -> Amount {
        let settlement: sqlx::types::Json<Settlement> = sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(method.id).fetch_one(&self.pool).await.unwrap();
        match settlement.0 {
            Settlement::Prepaid { balance, .. } => balance,
            Settlement::OnSpot => Amount::Uses(0),
        }
    }
}

fn pass(uses: u32, valid: Option<Interval>) -> Settlement {
    Settlement::Prepaid { balance: Amount::Uses(uses), valid }
}

#[sqlx::test]
async fn a_method_whose_filter_holds_for_the_service_pays(pool: PgPool) {
    let w = world(pool).await;
    let method = w.method("attributes.category = \"massage\"", pass(10, None)).await;
    w.pay_with(&method, Amount::Uses(1), 9).await.unwrap();
    assert_eq!(w.uses_left(&method).await, Amount::Uses(9));
}

#[sqlx::test]
async fn a_method_whose_filter_fails_for_the_service_does_not_pay_and_nothing_is_booked(pool: PgPool) {
    let w = world(pool.clone()).await;
    let method = w.method("attributes.category = \"facial\"", pass(10, None)).await;
    let result = w.pay_with(&method, Amount::Uses(1), 9).await;
    assert!(matches!(result, Err(Error::NotEligible(id)) if id == method.id));
    assert_eq!(w.uses_left(&method).await, Amount::Uses(10));
    let booked: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments").fetch_one(&pool).await.unwrap();
    assert_eq!(booked, 0);
}

#[sqlx::test]
async fn the_filter_may_look_at_the_name_and_the_mode_too(pool: PgPool) {
    let w = world(pool).await;
    let by_name = w.method("display_name = \"mass*\"", Settlement::OnSpot).await;
    let by_mode = w.method("mode = \"class\"", Settlement::OnSpot).await;
    w.pay_with(&by_name, Amount::Uses(0), 9).await.unwrap();
    assert!(matches!(w.pay_with(&by_mode, Amount::Uses(0), 10).await, Err(Error::NotEligible(_))));
}

#[sqlx::test]
async fn a_pass_is_judged_by_the_day_of_the_appointment_not_the_day_of_booking(pool: PgPool) {
    let w = world(pool).await;
    let covering = Interval { start: day(0), end: day(30) };
    let before = Interval { start: day(-60), end: day(-30) };
    let after = Interval { start: day(10), end: day(40) };
    let now_valid = w.method("attributes.category = \"massage\"", pass(5, Some(covering))).await;
    w.pay_with(&now_valid, Amount::Uses(1), 9).await.unwrap();
    assert_eq!(w.uses_left(&now_valid).await, Amount::Uses(4));
    for (index, window) in [before, after].into_iter().enumerate() {
        let method = w.method("attributes.category = \"massage\"", pass(5, Some(window))).await;
        let result = w.pay_with(&method, Amount::Uses(1), 14 + index as u32).await;
        assert!(matches!(result, Err(Error::Expired(id)) if id == method.id), "window {index}");
        assert_eq!(w.uses_left(&method).await, Amount::Uses(5));
    }
}

#[sqlx::test]
async fn a_balance_of_money_pays_until_it_runs_out(pool: PgPool) {
    let w = world(pool).await;
    let money = |micros| Amount::Money(Price { micros, currency: "USD".into() });
    let voucher = w.method("attributes.category = \"massage\"", Settlement::Prepaid { balance: money(100_000_000), valid: None }).await;
    w.pay_with(&voucher, money(30_000_000), 9).await.unwrap();
    assert_eq!(w.uses_left(&voucher).await, money(70_000_000));
    assert!(matches!(w.pay_with(&voucher, money(150_000_000), 10).await, Err(Error::Insufficient(_))));
    assert!(matches!(w.pay_with(&voucher, Amount::Uses(1), 11).await, Err(Error::Insufficient(_))));
}
