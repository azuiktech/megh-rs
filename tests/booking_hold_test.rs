//! Specification: a hold is an appointment with the status `held` and an `expires_at`. It uses a room's capacity, never
//! a place of its waitlist; it is booked, cancelled or, when it runs out, ignored by every query and then cancelled by
//! `expire_holds`.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{Appointments, AppointmentRow, Error, IdempotencyKey, PaymentMethodRow, Reservations, Search, ServiceRow, Slot, Slots, Status};
use sqlx::PgPool;
use uuid::Uuid;

struct Hall {
    pool: PgPool,
    class: ServiceRow,
    on_spot: PaymentMethodRow,
    slot: Slot,
}

async fn hall(pool: PgPool, waitlist: u32) -> Hall {
    let org = org(&pool, "hall").await;
    let room = bookable_with_waitlist(&pool, org, "room", 1, waitlist, &[("kind", "room")]).await;
    open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let class = service(&pool, org, "class", 60, None, is("kind", "room")).await;
    let slots = Slots::new(pool.clone()).find(&request(class.id, whole_day(1))).await.unwrap();
    let slot = slots.into_iter().find(|slot| slot.allocations[0].during.start == at(1, 9, 0)).expect("a slot at nine");
    let on_spot = on_spot(&pool, org).await;
    Hall { pool, class, on_spot, slot }
}

impl Hall {
    fn reservations(&self) -> Reservations {
        Reservations::new(self.pool.clone())
    }

    async fn hold(&self) -> megh::booking::Result<AppointmentRow> {
        let client = client(&self.pool).await;
        let mut held = self.reservations().hold(&request_for(client.id, self.class.id, whole_day(1)), &self.slot, Utc::now() + Duration::minutes(10)).await?;
        Ok(held.remove(0))
    }

    async fn book(&self) -> megh::booking::Result<AppointmentRow> {
        let client = client(&self.pool).await;
        let payment = pays(&client, &self.on_spot);
        let key = IdempotencyKey(Uuid::new_v4().to_string());
        let mut booked = self.reservations().book(&request_for(client.id, self.class.id, whole_day(1)), &self.slot, vec![payment], key).await?;
        Ok(booked.remove(0))
    }

    async fn offered(&self) -> Vec<Status> {
        let slots = Slots::new(self.pool.clone()).find(&request(self.class.id, whole_day(1))).await.unwrap();
        slots.iter().filter(|slot| slot.allocations[0].during.start == at(1, 9, 0)).map(|slot| slot.allocations[0].status).collect()
    }

    async fn expire(&self, hold: &AppointmentRow) {
        sqlx::query("UPDATE appointments SET expires_at = now() - interval '1 minute' WHERE id = $1").bind(hold.id).execute(&self.pool).await.unwrap();
    }
}

#[sqlx::test]
async fn a_hold_is_a_held_appointment_that_expires_and_uses_capacity(pool: PgPool) {
    let h = hall(pool, 1).await;
    let hold = h.hold().await.unwrap();
    assert_eq!((hold.status, hold.expires_at.is_some(), hold.payment.is_none()), (Status::Held, true, true));
    assert_eq!(h.offered().await, vec![Status::Waitlist]);
}

#[sqlx::test]
async fn a_hold_never_takes_a_place_of_the_waitlist(pool: PgPool) {
    let h = hall(pool, 1).await;
    h.book().await.unwrap();
    assert!(matches!(h.hold().await, Err(Error::Occupied(_))));
}

#[sqlx::test]
async fn an_expired_hold_is_ignored_at_once_and_cancelled_by_expire_holds(pool: PgPool) {
    let h = hall(pool, 0).await;
    let hold = h.hold().await.unwrap();
    assert!(h.offered().await.is_empty());
    h.expire(&hold).await;
    assert_eq!(h.offered().await, vec![Status::Booked]);
    assert_eq!(h.reservations().expire_holds().await.unwrap(), 1);
    let status: Status = sqlx::query_scalar("SELECT status FROM appointments WHERE id = $1").bind(hold.id).fetch_one(&h.pool).await.unwrap();
    assert_eq!(status, Status::Cancelled);
    assert_eq!(h.reservations().expire_holds().await.unwrap(), 0);
}

#[sqlx::test]
async fn a_held_appointment_can_be_cancelled_and_frees_its_capacity(pool: PgPool) {
    let h = hall(pool, 0).await;
    let hold = h.hold().await.unwrap();
    h.reservations().transition(hold.id, Status::Cancelled).await.unwrap();
    assert_eq!(h.offered().await, vec![Status::Booked]);
}

#[sqlx::test]
async fn a_held_appointment_is_booked_with_its_payment_and_only_once(pool: PgPool) {
    let h = hall(pool, 0).await;
    let hold = h.hold().await.unwrap();
    let payment = pays(&client_of(&h.pool, hold.org_id).await, &h.on_spot);
    let booked = h.reservations().book_held(hold.id, payment.clone(), IdempotencyKey("k".into())).await.unwrap();
    assert_eq!((booked.status, booked.expires_at, booked.payment.clone()), (Status::Booked, None, Some(payment.clone())));
    assert!(matches!(h.reservations().book_held(hold.id, payment, IdempotencyKey("other".into())).await, Err(Error::Transition(Status::Booked, Status::Booked))));
}

#[sqlx::test]
async fn an_expired_hold_cannot_be_booked(pool: PgPool) {
    let h = hall(pool, 0).await;
    let hold = h.hold().await.unwrap();
    h.expire(&hold).await;
    let payment = pays(&client_of(&h.pool, hold.org_id).await, &h.on_spot);
    assert!(matches!(h.reservations().book_held(hold.id, payment, IdempotencyKey("k".into())).await, Err(Error::HoldExpired(_))));
}

#[sqlx::test]
async fn a_hold_is_not_promoted_by_a_status_change(pool: PgPool) {
    let h = hall(pool, 0).await;
    let hold = h.hold().await.unwrap();
    assert!(matches!(h.reservations().transition(hold.id, Status::Booked).await, Err(Error::Transition(Status::Held, Status::Booked))));
}
