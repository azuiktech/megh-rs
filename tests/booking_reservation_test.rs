//! Specification: docs/TPD-booking.md, sections 12 (hold, booking, cancel) and 13 (payment).

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{
    Allocation, Amount, BookableRow, Appointments, ClientRow, Confirmation, Error, HoldRow, IdempotencyKey, Payment, PaymentMethod,
    PaymentMethodId, PaymentMethodRow, Reservations, Search, ServiceRow, Settlement, Slot, Slots,
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
    let haircut = service(&pool, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    open_rule(&pool, Where { service: Some(haircut.id), ..Where::default() }, 9, 8).await;
    let client = client(&pool).await;
    World { pool, room, haircut, client }
}

impl World {
    async fn first_slot(&self) -> Slot {
        let request = request(self.haircut.id, whole_day(1));
        Slots::new(self.pool.clone()).find(&request).await.unwrap().remove(0)
    }

    async fn hold(&self, slot: &Slot) -> megh::booking::Result<HoldRow> {
        let request = request(self.haircut.id, whole_day(1));
        Reservations::new(self.pool.clone()).hold(&request, slot, Utc::now() + Duration::minutes(10)).await
    }

    fn confirmation(&self) -> Confirmation {
        Confirmation { clients: vec![self.client.id], payer: self.client.id, payments: vec![], attributes: Default::default() }
    }

    async fn confirm(&self, hold: &HoldRow, confirmation: Confirmation, key: &str) -> megh::booking::Result<megh::booking::AppointmentRow> {
        Reservations::new(self.pool.clone()).confirm(hold.id, confirmation, IdempotencyKey(key.into())).await
    }

    async fn slots(&self) -> Vec<Slot> {
        let request = request(self.haircut.id, whole_day(1));
        Slots::new(self.pool.clone()).find(&request).await.unwrap()
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
    assert!(hold.expires_at > Utc::now());
    assert!(!blocked(&w.slots().await, &slot.allocations));
}

#[sqlx::test]
async fn a_second_hold_of_a_taken_slot_is_occupied(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    w.hold(&slot).await.unwrap();
    assert!(matches!(w.hold(&slot).await, Err(Error::Occupied(_))));
}

#[sqlx::test]
async fn a_hold_outside_open_time_is_refused(pool: PgPool) {
    let w = world(pool).await;
    let mut slot = w.first_slot().await;
    slot.allocations.iter_mut().for_each(|a| a.during = between(1, 20, 21));
    assert!(matches!(w.hold(&slot).await, Err(Error::NotOpen(_))));
}

#[sqlx::test]
async fn confirming_a_hold_books_it_and_the_booking_owns_the_time(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    let hold = w.hold(&slot).await.unwrap();
    let booking = w.confirm(&hold, w.confirmation(), "key-1").await.unwrap();
    assert_eq!(booking.payer_id, w.client.id);
    let owners: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as("SELECT hold_id, appointment_id FROM occupancies WHERE deleted_at IS NULL").fetch_all(&w.pool).await.unwrap();
    assert!(!owners.is_empty());
    assert!(owners.iter().all(|(hold_id, appointment_id)| hold_id.is_none() && *appointment_id == Some(booking.id.0)));
    assert!(!blocked(&w.slots().await, &slot.allocations));
}

#[sqlx::test]
async fn confirming_twice_with_the_same_key_returns_the_same_booking(pool: PgPool) {
    let w = world(pool).await;
    let hold = w.hold(&w.first_slot().await).await.unwrap();
    let first = w.confirm(&hold, w.confirmation(), "same").await.unwrap();
    let again = w.confirm(&hold, w.confirmation(), "same").await.unwrap();
    assert_eq!(first.id, again.id);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments").fetch_one(&w.pool).await.unwrap();
    assert_eq!(count, 1);
}

#[sqlx::test]
async fn the_same_key_with_another_request_is_refused(pool: PgPool) {
    let w = world(pool).await;
    let hold = w.hold(&w.first_slot().await).await.unwrap();
    w.confirm(&hold, w.confirmation(), "same").await.unwrap();
    let other_payer = client(&w.pool).await;
    let changed = Confirmation { payer: other_payer.id, ..w.confirmation() };
    assert!(matches!(w.confirm(&hold, changed, "same").await, Err(Error::KeyReused)));
}

#[sqlx::test]
async fn an_expired_hold_cannot_be_confirmed(pool: PgPool) {
    let w = world(pool).await;
    let expired = held(&w.pool, w.room.id, between(1, 9, 10), Utc::now() - Duration::minutes(1)).await;
    let result = w.confirm(&expired, w.confirmation(), "late").await;
    assert!(matches!(result, Err(Error::HoldExpired(id)) if id == expired.id));
}

#[sqlx::test]
async fn cancelling_soft_deletes_the_booking_and_frees_the_time(pool: PgPool) {
    let w = world(pool).await;
    let slot = w.first_slot().await;
    let hold = w.hold(&slot).await.unwrap();
    let booking = w.confirm(&hold, w.confirmation(), "key").await.unwrap();
    Reservations::new(w.pool.clone()).cancel(booking.id).await.unwrap();
    let deleted: Option<chrono::DateTime<Utc>> = sqlx::query_scalar("SELECT deleted_at FROM appointments WHERE id = $1").bind(booking.id).fetch_one(&w.pool).await.unwrap();
    assert!(deleted.is_some());
    assert!(w.slots().await.iter().any(|candidate| candidate.allocations == slot.allocations));
}

#[sqlx::test]
async fn a_prepaid_method_is_debited_by_the_booking(pool: PgPool) {
    let w = world(pool).await;
    let method = PaymentMethod {
        org_id: None,
        name: "30-visit pass".into(),
        owner_id: Some(w.client.id),
        eligible: is("kind", "any"),
        settlement: Settlement::Prepaid { balance: Amount::Uses(30), valid: None },
        attributes: Default::default(),
    };
    let method = PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), method).insert(&w.pool).await.unwrap();
    let hold = w.hold(&w.first_slot().await).await.unwrap();
    let paid = Confirmation { payments: vec![Payment { method: method.id, amount: Amount::Uses(1) }], ..w.confirmation() };
    w.confirm(&hold, paid, "pass").await.unwrap();
    let settlement: sqlx::types::Json<Settlement> = sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(method.id).fetch_one(&w.pool).await.unwrap();
    assert_eq!(settlement.0, Settlement::Prepaid { balance: Amount::Uses(29), valid: None });
}
