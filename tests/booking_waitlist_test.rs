//! Specification: a room has a `capacity` and a `waitlist`. An appointment takes capacity while the room has any;
//! over capacity but within the waitlist it is a `waitlist` appointment; otherwise the room is occupied. Status
//! changes follow one table, a payment is debited only on entering `booked`, and a cancelled appointment frees the
//! place it held: capacity for `booked` and `confirmed`, a waitlist place for `waitlist`.

mod booking_support;

use booking_support::*;
use megh::booking::{
    Amount, AppointmentRow, Appointments, Error, IdempotencyKey, Payment, PaymentMethod, PaymentMethodId, PaymentMethodRow, Reservations, Search, ServiceRow, Settlement, Slot, Slots, Status,
};
use sqlx::PgPool;
use uuid::Uuid;

struct Hall {
    pool: PgPool,
    class: ServiceRow,
    on_spot: PaymentMethodRow,
    /// The slot at nine on day one, found before anything was booked.
    slot: Slot,
}

/// A room of capacity 1 and waitlist 1, open 9 to 17, and a service that needs only the room.
async fn hall(pool: PgPool) -> Hall {
    let org = org(&pool, "hall").await;
    let room = bookable_with_waitlist(&pool, org, "room", 1, 1, &[("kind", "room")]).await;
    open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let class = service(&pool, org, "class", 60, None, is("kind", "room")).await;
    let slots = Slots::new(pool.clone()).find(&request(class.id, whole_day(1))).await.unwrap();
    let slot = slots.into_iter().find(|slot| slot.allocations[0].during.start == at(1, 9, 0)).expect("a slot at nine");
    let on_spot = on_spot(&pool, org).await;
    Hall { pool, class, on_spot, slot }
}

impl Hall {
    /// A new client books the slot, paying with the pass or, without one, on the spot.
    async fn book(&self, pass: Option<&PaymentMethodRow>) -> megh::booking::Result<AppointmentRow> {
        let client = client(&self.pool).await;
        let payment = Payment { payer_id: client.id, method: pass.unwrap_or(&self.on_spot).id, amount: Amount::Uses(1) };
        let key = IdempotencyKey(Uuid::new_v4().to_string());
        let mut booked = Reservations::new(self.pool.clone()).book(&request_for(client.id, self.class.id, whole_day(1)), &self.slot, vec![payment], key).await?;
        Ok(booked.remove(0))
    }

    async fn transition(&self, appointment: &AppointmentRow, to: Status) -> megh::booking::Result<AppointmentRow> {
        Reservations::new(self.pool.clone()).transition(appointment.id, to).await
    }

    async fn cancel(&self, appointment: &AppointmentRow) {
        self.transition(appointment, Status::Cancelled).await.unwrap();
    }

    async fn statuses_offered(&self) -> Vec<Status> {
        let request = request(self.class.id, whole_day(1));
        let slots = Slots::new(self.pool.clone()).find(&request).await.unwrap();
        slots.iter().filter(|slot| slot.allocations[0].during.start == at(1, 9, 0)).map(|slot| slot.allocations[0].status).collect()
    }
}

#[sqlx::test]
async fn an_appointment_is_booked_while_the_room_has_capacity_then_waitlisted_then_refused(pool: PgPool) {
    let h = hall(pool).await;
    assert_eq!(h.book(None).await.unwrap().status, Status::Booked);
    assert_eq!(h.book(None).await.unwrap().status, Status::Waitlist);
    assert!(matches!(h.book(None).await, Err(Error::Occupied(_))));
}

#[sqlx::test]
async fn a_search_offers_the_waitlist_slots_with_their_status(pool: PgPool) {
    let h = hall(pool).await;
    assert_eq!(h.statuses_offered().await, vec![Status::Booked]);
    h.book(None).await.unwrap();
    assert_eq!(h.statuses_offered().await, vec![Status::Waitlist]);
    h.book(None).await.unwrap();
    assert!(h.statuses_offered().await.is_empty());
}

#[sqlx::test]
async fn cancelling_a_waitlisted_appointment_frees_the_waitlist_and_not_the_capacity(pool: PgPool) {
    let h = hall(pool).await;
    h.book(None).await.unwrap();
    let waiting = h.book(None).await.unwrap();
    h.cancel(&waiting).await;
    assert_eq!(h.statuses_offered().await, vec![Status::Waitlist]);
    assert_eq!(h.book(None).await.unwrap().status, Status::Waitlist);
}

#[sqlx::test]
async fn cancelling_a_booked_appointment_frees_the_capacity(pool: PgPool) {
    let h = hall(pool).await;
    let first = h.book(None).await.unwrap();
    h.book(None).await.unwrap();
    h.cancel(&first).await;
    assert_eq!(h.statuses_offered().await, vec![Status::Booked]);
}

#[sqlx::test]
async fn a_waitlisted_appointment_is_booked_only_when_there_is_capacity(pool: PgPool) {
    let h = hall(pool).await;
    let first = h.book(None).await.unwrap();
    let waiting = h.book(None).await.unwrap();
    assert!(matches!(h.transition(&waiting, Status::Booked).await, Err(Error::Occupied(_))));
    h.cancel(&first).await;
    assert_eq!(h.transition(&waiting, Status::Booked).await.unwrap().status, Status::Booked);
    assert_eq!(h.statuses_offered().await, vec![Status::Waitlist]);
}

#[sqlx::test]
async fn a_confirmed_appointment_still_holds_its_capacity(pool: PgPool) {
    let h = hall(pool).await;
    let first = h.book(None).await.unwrap();
    h.transition(&first, Status::Confirmed).await.unwrap();
    assert_eq!(h.statuses_offered().await, vec![Status::Waitlist]);
}

async fn pass(h: &Hall, uses: u32) -> PaymentMethodRow {
    let owner = client(&h.pool).await;
    let method = PaymentMethod {
        org_id: owner.org_id,
        name: "pass".into(),
        owner_id: Some(owner.id),
        eligible: is("mode", "appointment"),
        settlement: Settlement::Prepaid { balance: Amount::Uses(uses), valid: None },
        attributes: serde_json::json!({}),
    };
    PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), method).insert(&h.pool).await.unwrap()
}

async fn uses_left(h: &Hall, method: &PaymentMethodRow) -> serde_json::Value {
    sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(method.id).fetch_one(&h.pool).await.unwrap()
}

fn uses(count: u32) -> serde_json::Value {
    serde_json::to_value(Settlement::Prepaid { balance: Amount::Uses(count), valid: None }).unwrap()
}

#[sqlx::test]
async fn a_pass_is_debited_when_the_appointment_is_booked_and_not_while_it_waits(pool: PgPool) {
    let h = hall(pool).await;
    let pass = pass(&h, 3).await;
    let first = h.book(None).await.unwrap();
    let waiting = h.book(Some(&pass)).await.unwrap();
    assert_eq!((waiting.status, uses_left(&h, &pass).await), (Status::Waitlist, uses(3)));
    h.cancel(&first).await;
    h.transition(&waiting, Status::Booked).await.unwrap();
    assert_eq!(uses_left(&h, &pass).await, uses(2));
}

#[sqlx::test]
async fn a_booking_with_a_pass_that_is_empty_books_nothing(pool: PgPool) {
    let h = hall(pool).await;
    let empty = pass(&h, 0).await;
    let result = h.book(Some(&empty)).await;
    assert!(matches!(result, Err(Error::Insufficient(method)) if method == empty.id));
    assert_eq!(h.statuses_offered().await, vec![Status::Booked]);
}

#[sqlx::test]
async fn only_the_listed_status_changes_are_allowed(pool: PgPool) {
    let h = hall(pool).await;
    let first = h.book(None).await.unwrap();
    let waiting = h.book(None).await.unwrap();
    for (appointment, to) in [(&waiting, Status::Confirmed), (&first, Status::Waitlist), (&first, Status::Booked), (&waiting, Status::Waitlist)] {
        assert!(matches!(h.transition(appointment, to).await, Err(Error::Transition(_, _))), "{:?}", to);
    }
    let confirmed = h.transition(&first, Status::Confirmed).await.unwrap();
    assert!(matches!(h.transition(&confirmed, Status::Booked).await, Err(Error::Transition(_, _))));
    h.cancel(&confirmed).await;
    assert!(matches!(h.transition(&confirmed, Status::Booked).await, Err(Error::Transition(_, _))));
    assert!(matches!(h.transition(&confirmed, Status::Cancelled).await, Err(Error::Transition(_, _))));
}
