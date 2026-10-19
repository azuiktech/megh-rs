//! Specification: an appointment is one client having one service, in one room with zero or more providers, from the
//! start to the end of the client's time. A slot is one appointment for each of its placements.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{
    Amount, AppointmentRow, Appointments, BookableRow, ClientRow, Error, IdempotencyKey, Payment, PaymentMethod, PaymentMethodId, PaymentMethodRow, Reservations, Search, ServiceRow, Settlement, Slot, Slots,
    Status,
};
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    pool: PgPool,
    org: Uuid,
    room: BookableRow,
    stylist: BookableRow,
    haircut: ServiceRow,
    client: ClientRow,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "salon").await;
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await;
    for found in [&room, &stylist] {
        open_rule(&pool, Where { bookable: Some(found.id), ..Where::default() }, 9, 8).await;
    }
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let client = client_of(&pool, org).await;
    World { pool, org, room, stylist, haircut, client }
}

impl World {
    fn request(&self) -> megh::booking::SearchRequest {
        request_for(self.client.id, self.haircut.id, whole_day(1))
    }

    async fn first_slot(&self) -> Slot {
        Slots::new(self.pool.clone()).find(&self.request()).await.unwrap().remove(0)
    }

    async fn held(&self) -> (Slot, AppointmentRow) {
        let slot = self.first_slot().await;
        let mut hold = Reservations::new(self.pool.clone()).hold(&self.request(), &slot, Utc::now() + Duration::minutes(10)).await.unwrap();
        (slot, hold.remove(0))
    }

    fn payment(&self, amount: Amount, method: PaymentMethodId) -> Payment {
        Payment { payer_id: self.client.id, method, amount }
    }

    async fn pass(&self, uses: u32) -> PaymentMethodRow {
        let pass = PaymentMethod {
            org_id: self.org,
            name: "pass".into(),
            owner_id: Some(self.client.id),
            eligible: is("mode", "appointment"),
            settlement: Settlement::Prepaid { balance: Amount::Uses(uses), valid: None },
            attributes: serde_json::json!({}),
        };
        PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), pass).insert(&self.pool).await.unwrap()
    }

    async fn book_held(&self, hold: &AppointmentRow, payment: Payment) -> megh::booking::Result<AppointmentRow> {
        Reservations::new(self.pool.clone()).book_held(hold.id, payment, IdempotencyKey(Uuid::new_v4().to_string())).await
    }
}

#[sqlx::test]
async fn a_hold_is_an_appointment_for_the_client_with_the_room_and_the_providers_of_its_slot(pool: PgPool) {
    let w = world(pool).await;
    let (slot, hold) = w.held().await;
    assert_eq!((hold.org_id, hold.client_id, hold.service_id, hold.status), (w.org, w.client.id, w.haircut.id, Status::Held));
    assert_eq!((hold.room_id, hold.provider_ids.clone()), (w.room.id, vec![w.stylist.id]));
    assert_eq!((hold.starts_at, hold.ends_at), (slot.placements[0].during.start, slot.placements[0].during.end));
    assert!(hold.expires_at.is_some() && hold.payment.is_none());
}

#[sqlx::test]
async fn booking_a_hold_keeps_the_appointment_and_records_its_payment(pool: PgPool) {
    let w = world(pool).await;
    let (_slot, hold) = w.held().await;
    let pass = w.pass(5).await;
    let payment = w.payment(Amount::Uses(1), pass.id);
    let booked = w.book_held(&hold, payment.clone()).await.unwrap();
    assert_eq!((booked.id, booked.status, booked.expires_at, booked.payment.clone()), (hold.id, Status::Booked, None, Some(payment)));
    assert_eq!((booked.client_id, booked.room_id, booked.starts_at), (hold.client_id, hold.room_id, hold.starts_at));
}

#[sqlx::test]
async fn the_appointment_times_are_the_clients_time(pool: PgPool) {
    let w = world(pool).await;
    let (_slot, hold) = w.held().await;
    assert_eq!(hold.ends_at - hold.starts_at, Duration::minutes(60));
}

#[sqlx::test]
async fn an_empty_prepaid_method_refuses_the_payment_and_books_nothing(pool: PgPool) {
    let w = world(pool).await;
    let pass = w.pass(0).await;
    let (_slot, hold) = w.held().await;
    let result = w.book_held(&hold, w.payment(Amount::Uses(1), pass.id)).await;
    assert!(matches!(result, Err(Error::Insufficient(method)) if method == pass.id));
    let booked: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments WHERE status = 'booked'").fetch_one(&w.pool).await.unwrap();
    assert_eq!(booked, 0);
}

#[sqlx::test]
async fn booking_a_slot_directly_makes_one_appointment_and_debits_the_pass(pool: PgPool) {
    let w = world(pool).await;
    let pass = w.pass(2).await;
    let slot = w.first_slot().await;
    let payments = vec![w.payment(Amount::Uses(1), pass.id)];
    let booked = Reservations::new(w.pool.clone()).book(&w.request(), &slot, payments, IdempotencyKey("direct".into())).await.unwrap();
    assert_eq!(booked.len(), 1);
    assert_eq!((booked[0].status, booked[0].client_id), (Status::Booked, w.client.id));
    let left: serde_json::Value = sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(pass.id).fetch_one(&w.pool).await.unwrap();
    assert_eq!(left, serde_json::to_value(Settlement::Prepaid { balance: Amount::Uses(1), valid: None }).unwrap());
}

#[sqlx::test]
async fn the_same_key_returns_the_same_appointments_and_another_request_is_refused(pool: PgPool) {
    let w = world(pool).await;
    let pass = w.pass(5).await;
    let slot = w.first_slot().await;
    let payments = || vec![w.payment(Amount::Uses(1), pass.id)];
    let reservations = Reservations::new(w.pool.clone());
    let first = reservations.book(&w.request(), &slot, payments(), IdempotencyKey("same".into())).await.unwrap();
    let again = reservations.book(&w.request(), &slot, payments(), IdempotencyKey("same".into())).await.unwrap();
    assert_eq!(first[0].id, again[0].id);
    let other = vec![w.payment(Amount::Uses(2), pass.id)];
    assert!(matches!(reservations.book(&w.request(), &slot, other, IdempotencyKey("same".into())).await, Err(Error::KeyReused)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments").fetch_one(&w.pool).await.unwrap();
    assert_eq!(count, 1);
}
