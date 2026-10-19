//! Specification: a hold blocks its slot until it expires or is cancelled; booking it keeps the time; a key makes the
//! booking safe to retry; a payment is debited when the appointment is booked.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{
    Allocation, Amount, AppointmentRow, Appointments, BookableRow, ClientRow, Error, IdempotencyKey, Payment, PaymentMethod, PaymentMethodId, PaymentMethodRow, Reservations, Search, ServiceRow, Settlement,
    Slot, Slots, SearchRequest,
};
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    pool: PgPool,
    room: BookableRow,
    haircut: ServiceRow,
    client: ClientRow,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "salon").await;
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await;
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    open_rule(&pool, Where { service: Some(haircut.id), ..Where::default() }, 9, 8).await;
    let client = client_of(&pool, org).await;
    World { pool, room, haircut, client }
}

impl World {
    fn request(&self) -> SearchRequest {
        request_for(self.client.id, self.haircut.id, whole_day(1))
    }

    async fn first_slot(&self) -> Slot {
        Slots::new(self.pool.clone()).find(&self.request()).await.unwrap().remove(0)
    }

    async fn hold(&self, slot: &Slot) -> megh::booking::Result<AppointmentRow> {
        self.hold_for(&self.client, slot).await
    }

    async fn hold_for(&self, client: &ClientRow, slot: &Slot) -> megh::booking::Result<AppointmentRow> {
        let request = request_for(client.id, self.haircut.id, whole_day(1));
        let mut held = Reservations::new(self.pool.clone()).hold(&request, slot, Utc::now() + Duration::minutes(10)).await?;
        Ok(held.remove(0))
    }

    fn payment(&self, method: PaymentMethodId) -> Payment {
        Payment { payer_id: self.client.id, method, amount: Amount::Uses(1) }
    }

    async fn pass(&self, uses: u32) -> PaymentMethodRow {
        let method = PaymentMethod {
            org_id: self.room.org_id,
            name: "pass".into(),
            owner_id: Some(self.client.id),
            eligible: is("mode", "appointment"),
            settlement: Settlement::Prepaid { balance: Amount::Uses(uses), valid: None },
            attributes: serde_json::json!({}),
        };
        PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), method).insert(&self.pool).await.unwrap()
    }

    async fn book(&self, hold: &AppointmentRow, payment: Payment, key: &str) -> megh::booking::Result<AppointmentRow> {
        Reservations::new(self.pool.clone()).book_held(hold.id, payment, IdempotencyKey(key.into())).await
    }

    async fn slots(&self) -> Vec<Slot> {
        Slots::new(self.pool.clone()).find(&self.request()).await.unwrap()
    }
}

fn blocked(slots: &[Slot], allocations: &[Allocation]) -> bool {
    slots.iter().flat_map(|slot| &slot.allocations).any(|a| allocations.iter().any(|held| held.bookable == a.bookable && overlaps(&held.during, &a.during)))
}

#[sqlx::test]
async fn a_hold_blocks_its_slot_until_it_expires(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    let hold = w.hold(&slot).await.unwrap();
    assert!(hold.expires_at.unwrap() > Utc::now());
    assert!(!blocked(&w.slots().await, &slot.allocations));
}

#[sqlx::test]
async fn a_second_hold_of_a_taken_slot_is_occupied(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    w.hold(&slot).await.unwrap();
    let other = client_of(&w.pool, w.room.org_id).await;
    assert!(matches!(w.hold_for(&other, &slot).await, Err(Error::Occupied(_))));
}

#[sqlx::test]
async fn a_hold_outside_open_time_is_refused(pool: PgPool) {
    let w = world(pool).await;
    let mut slot = w.first_slot().await;
    slot.allocations.iter_mut().for_each(|a| a.during = between(1, 20, 21));
    assert!(matches!(w.hold(&slot).await, Err(Error::NotOpen(_))));
}

#[sqlx::test]
async fn booking_a_hold_keeps_its_time_for_the_appointment(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    let hold = w.hold(&slot).await.unwrap();
    let pass = w.pass(3).await;
    let booking = w.book(&hold, w.payment(pass.id), "key-1").await.unwrap();
    let owners: Vec<Uuid> = sqlx::query_scalar("SELECT appointment_id FROM occupancies WHERE deleted_at IS NULL").fetch_all(&w.pool).await.unwrap();
    assert!(!owners.is_empty() && owners.iter().all(|owner| *owner == booking.id.0));
    assert!(!blocked(&w.slots().await, &slot.allocations));
}

#[sqlx::test]
async fn booking_twice_with_the_same_key_returns_the_same_appointment(pool: PgPool) {
    let w = world(pool).await;
    let hold = w.hold(&w.first_slot().await).await.unwrap();
    let pass = w.pass(3).await;
    let first = w.book(&hold, w.payment(pass.id), "same").await.unwrap();
    let again = w.book(&hold, w.payment(pass.id), "same").await.unwrap();
    assert_eq!(first.id, again.id);
    let uses: serde_json::Value = sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(pass.id).fetch_one(&w.pool).await.unwrap();
    assert_eq!(uses, serde_json::to_value(Settlement::Prepaid { balance: Amount::Uses(2), valid: None }).unwrap());
}

#[sqlx::test]
async fn the_same_key_with_another_payment_is_refused(pool: PgPool) {
    let w = world(pool).await;
    let hold = w.hold(&w.first_slot().await).await.unwrap();
    let (pass, other) = (w.pass(3).await, w.pass(3).await);
    w.book(&hold, w.payment(pass.id), "same").await.unwrap();
    assert!(matches!(w.book(&hold, w.payment(other.id), "same").await, Err(Error::KeyReused)));
}

#[sqlx::test]
async fn an_expired_hold_cannot_be_booked(pool: PgPool) {
    let w = world(pool).await;
    let expired = held(&w.pool, w.room.id, between(1, 9, 10), Utc::now() - Duration::minutes(1)).await;
    let pass = w.pass(3).await;
    let result = w.book(&expired, w.payment(pass.id), "late").await;
    assert!(matches!(result, Err(Error::HoldExpired(id)) if id == expired.id));
}

#[sqlx::test]
async fn cancelling_soft_deletes_the_appointment_and_frees_the_time(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    let hold = w.hold(&slot).await.unwrap();
    let pass = w.pass(3).await;
    let booking = w.book(&hold, w.payment(pass.id), "key").await.unwrap();
    Reservations::new(w.pool.clone()).cancel(booking.id).await.unwrap();
    let deleted: Option<chrono::DateTime<Utc>> = sqlx::query_scalar("SELECT deleted_at FROM appointments WHERE id = $1").bind(booking.id).fetch_one(&w.pool).await.unwrap();
    assert!(deleted.is_some());
    assert!(w.slots().await.iter().any(|candidate| candidate.allocations == slot.allocations));
}

#[sqlx::test]
async fn a_prepaid_method_is_debited_by_the_booking(pool: PgPool) {
    let w = world(pool).await;
    let pass = w.pass(30).await;
    let hold = w.hold(&w.first_slot().await).await.unwrap();
    w.book(&hold, w.payment(pass.id), "pass").await.unwrap();
    let settlement: sqlx::types::Json<Settlement> = sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(pass.id).fetch_one(&w.pool).await.unwrap();
    assert_eq!(settlement.0, Settlement::Prepaid { balance: Amount::Uses(29), valid: None });
}
