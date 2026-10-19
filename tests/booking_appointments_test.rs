//! Specification: docs/TPD-booking.md, sections 12 and 13 and R3: what a confirmed hold keeps.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{
    Amount, AppointmentRow, Appointments, ClientRow, Confirmation, Error, HoldRow, IdempotencyKey, Payment, PaymentMethod, PaymentMethodId, PaymentMethodRow,
    Reservations, Search, ServiceRow, Settlement, Slot, Slots,
};
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    pool: PgPool,
    org: Uuid,
    haircut: ServiceRow,
    client: ClientRow,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "salon").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(&pool).await.unwrap();
    for (name, tag) in [("room", ("kind", "room")), ("stylist", ("role", "stylist"))] {
        let found = bookable(&pool, org, name, 1, &[tag]).await;
        open_rule(&pool, Where { bookable: Some(found.id), ..Where::default() }, 9, 8).await;
    }
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let client = client(&pool).await;
    World { pool, org, haircut, client }
}

impl World {
    async fn held(&self) -> (Slot, HoldRow) {
        let request = request(self.haircut.id, whole_day(1));
        let slot = Slots::new(self.pool.clone()).find(&request).await.unwrap().remove(0);
        let hold = Reservations::new(self.pool.clone()).hold(&request, &slot, Utc::now() + Duration::minutes(10)).await.unwrap();
        (slot, hold)
    }

    fn confirmation(&self, payments: Vec<Payment>) -> Confirmation {
        Confirmation { clients: vec![self.client.id], payer: self.client.id, payments, attributes: serde_json::json!({}) }
    }

    async fn confirm(&self, hold: &HoldRow, payments: Vec<Payment>) -> megh::booking::Result<AppointmentRow> {
        Reservations::new(self.pool.clone()).confirm(hold.id, self.confirmation(payments), IdempotencyKey("key".into())).await
    }
}

#[sqlx::test]
async fn a_hold_keeps_the_placements_of_its_slot(pool: PgPool) {
    let w = world(pool).await;
    let (slot, hold) = w.held().await;
    assert_eq!(hold.placements, slot.placements);
    assert_eq!(hold.org_id, w.org);
}

#[sqlx::test]
async fn the_appointment_keeps_the_id_of_its_hold(pool: PgPool) {
    let w = world(pool).await;
    let (_slot, hold) = w.held().await;
    let appointment = w.confirm(&hold, vec![]).await.unwrap();
    assert_eq!(appointment.id.0, hold.id.0);
}

#[sqlx::test]
async fn the_appointment_keeps_the_org_and_its_time_zone(pool: PgPool) {
    let w = world(pool).await;
    let (_slot, hold) = w.held().await;
    let appointment = w.confirm(&hold, vec![]).await.unwrap();
    assert_eq!((appointment.org_id, appointment.tz.as_str()), (w.org, "America/New_York"));
}

#[sqlx::test]
async fn each_line_has_its_clients_time_and_price(pool: PgPool) {
    let w = world(pool).await;
    let (_slot, hold) = w.held().await;
    let appointment = w.confirm(&hold, vec![]).await.unwrap();
    let line = &appointment.lines[0];
    assert_eq!((line.client, line.service, line.duration, line.price.micros), (w.client.id, w.haircut.id, minutes(60), 100_000_000));
}

#[sqlx::test]
async fn an_empty_prepaid_method_refuses_the_payment_and_books_nothing(pool: PgPool) {
    let w = world(pool).await;
    let pass = PaymentMethod {
        org_id: w.org,
        name: "empty pass".into(),
        owner_id: Some(w.client.id),
        eligible: is("mode", "appointment"),
        settlement: Settlement::Prepaid { balance: Amount::Uses(0), valid: None },
        attributes: serde_json::json!({}),
    };
    let pass = PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), pass).insert(&w.pool).await.unwrap();
    let (_slot, hold) = w.held().await;
    let result = w.confirm(&hold, vec![Payment { method: pass.id, amount: Amount::Uses(1) }]).await;
    assert!(matches!(result, Err(Error::Insufficient(method)) if method == pass.id));
    let booked: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments").fetch_one(&w.pool).await.unwrap();
    assert_eq!(booked, 0);
}
