//! Specification: an appointment is moved to another slot of the same client and service. It keeps its id, status and
//! payment; the old time is freed; the move may overlap the appointment's own old time.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{AppointmentId, AppointmentRow, Appointments, ClientRow, Error, IdempotencyKey, PaymentMethodRow, Reservations, Search, SearchRequest, ServiceRow, Slot, Slots, Status};
use sqlx::PgPool;
use uuid::Uuid;

struct Salon {
    pool: PgPool,
    haircut: ServiceRow,
    colour: ServiceRow,
    on_spot: PaymentMethodRow,
    ann: ClientRow,
    ben: ClientRow,
}

/// A stylist and a chair, open 9 to 17, a one-hour haircut and a one-hour colour.
async fn salon(pool: PgPool) -> Salon {
    let org = org(&pool, "salon").await;
    for (name, tag) in [("chair", ("kind", "chair")), ("stylist", ("role", "stylist"))] {
        let found = bookable(&pool, org, name, 1, &[tag]).await;
        open_rule(&pool, Where { bookable: Some(found.id), ..Where::default() }, 9, 8).await;
    }
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "chair")).await;
    let colour = service(&pool, org, "colour", 60, Some(is("role", "stylist")), is("kind", "chair")).await;
    let (ann, ben, on_spot) = (client_of(&pool, org).await, client_of(&pool, org).await, on_spot(&pool, org).await);
    Salon { pool, haircut, colour, on_spot, ann, ben }
}

impl Salon {
    fn request(&self, client: &ClientRow, service: &ServiceRow, from: u32, to: u32) -> SearchRequest {
        request_for(client.id, service.id, between(1, from, to))
    }

    async fn slot(&self, request: &SearchRequest) -> Slot {
        Slots::new(self.pool.clone()).find(request).await.unwrap().remove(0)
    }

    async fn book(&self, client: &ClientRow, service: &ServiceRow, from: u32) -> AppointmentRow {
        let request = self.request(client, service, from, from + 1);
        let slot = self.slot(&request).await;
        Reservations::new(self.pool.clone()).book(&request, &slot, vec![pays(client, &self.on_spot)], IdempotencyKey(Uuid::new_v4().to_string())).await.unwrap().remove(0)
    }

    async fn hold(&self, client: &ClientRow, from: u32) -> AppointmentRow {
        let request = self.request(client, &self.haircut, from, from + 1);
        let slot = self.slot(&request).await;
        Reservations::new(self.pool.clone()).hold(&request, &slot, Utc::now() + Duration::minutes(10)).await.unwrap().remove(0)
    }

    /// The slot of the search that the appointment's own time does not block.
    async fn slot_for(&self, appointment: &AppointmentRow, request: &SearchRequest) -> Slot {
        Slots::new(self.pool.clone()).find_ignoring(request, appointment.id.0).await.unwrap().remove(0)
    }

    async fn move_to(&self, appointment: &AppointmentRow, client: &ClientRow, service: &ServiceRow, from: u32) -> megh::booking::Result<AppointmentRow> {
        let request = self.request(client, service, from, from + 1);
        let slot = self.slot_for(appointment, &request).await;
        Reservations::new(self.pool.clone()).reschedule(appointment.id, &request, &slot).await
    }

    async fn starts(&self, client: &ClientRow) -> Vec<chrono::DateTime<Utc>> {
        let slots = Slots::new(self.pool.clone()).find(&self.request(client, &self.haircut, 9, 17)).await.unwrap();
        slots.iter().map(|slot| slot.placements[0].during.start).collect()
    }
}

#[sqlx::test]
async fn an_appointment_moves_to_another_time_keeping_its_id_client_and_service(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.ann, &s.haircut, 9).await;
    let moved = s.move_to(&booked, &s.ann, &s.haircut, 14).await.unwrap();
    assert_eq!((moved.id, moved.client_id, moved.service_id, moved.status), (booked.id, booked.client_id, booked.service_id, Status::Booked));
    assert_eq!((moved.starts_at, moved.ends_at), (at(1, 14, 0), at(1, 15, 0)));
}

#[sqlx::test]
async fn the_old_time_is_free_again_and_the_new_one_is_taken(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.ann, &s.haircut, 9).await;
    s.move_to(&booked, &s.ann, &s.haircut, 14).await.unwrap();
    let starts = s.starts(&s.ben).await;
    assert!(starts.contains(&at(1, 9, 0)) && !starts.contains(&at(1, 14, 0)));
}

#[sqlx::test]
async fn a_move_may_overlap_the_appointments_own_old_time(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.ann, &s.haircut, 9).await;
    let request = request_for(s.ann.id, s.haircut.id, between(1, 9, 11));
    let slot = Slots::new(s.pool.clone()).find_ignoring(&request, booked.id.0).await.unwrap().into_iter().find(|slot| slot.placements[0].during.start == at(1, 9, 30)).expect("a slot at half past nine");
    let moved = Reservations::new(s.pool.clone()).reschedule(booked.id, &request, &slot).await.unwrap();
    assert_eq!(moved.starts_at, at(1, 9, 30));
}

#[sqlx::test]
async fn a_slot_that_was_taken_in_the_meantime_is_refused_and_nothing_moves(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.ann, &s.haircut, 9).await;
    let request = s.request(&s.ann, &s.haircut, 14, 15);
    let slot = s.slot_for(&booked, &request).await;
    s.book(&s.ben, &s.haircut, 14).await;
    let result = Reservations::new(s.pool.clone()).reschedule(booked.id, &request, &slot).await;
    assert!(matches!(result, Err(Error::Occupied(_)) | Err(Error::ClientBusy(_))), "{result:?}");
    let still: AppointmentRow = sqlx::query_as("SELECT * FROM appointments WHERE id = $1").bind(booked.id).fetch_one(&s.pool).await.unwrap();
    assert_eq!(still.starts_at, at(1, 9, 0));
}

#[sqlx::test]
async fn the_slot_must_be_for_the_same_service_and_client(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.ann, &s.haircut, 9).await;
    assert!(matches!(s.move_to(&booked, &s.ann, &s.colour, 14).await, Err(Error::Invalid(_))));
    assert!(matches!(s.move_to(&booked, &s.ben, &s.haircut, 14).await, Err(Error::Invalid(_))));
}

#[sqlx::test]
async fn a_cancelled_appointment_cannot_be_moved(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.ann, &s.haircut, 9).await;
    Reservations::new(s.pool.clone()).cancel(booked.id).await.unwrap();
    assert!(matches!(s.move_to(&booked, &s.ann, &s.haircut, 14).await, Err(Error::NotFound)));
}

#[sqlx::test]
async fn a_held_appointment_moves_and_keeps_its_expiry(pool: PgPool) {
    let s = salon(pool).await;
    let hold = s.hold(&s.ann, 9).await;
    let moved = s.move_to(&hold, &s.ann, &s.haircut, 14).await.unwrap();
    assert_eq!((moved.status, moved.expires_at, moved.starts_at), (Status::Held, hold.expires_at, at(1, 14, 0)));
}

#[sqlx::test]
async fn the_client_cannot_be_moved_onto_a_time_they_already_have(pool: PgPool) {
    let s = salon(pool).await;
    let first = s.book(&s.ann, &s.haircut, 9).await;
    let second = s.book(&s.ann, &s.colour, 14).await;
    let request = s.request(&s.ann, &s.colour, 9, 10);
    let slot = Slots::new(s.pool.clone()).find_ignoring(&request, second.id.0).await;
    let slot = slot.unwrap().into_iter().next();
    assert!(slot.is_none(), "the client already has nine to ten with the first appointment: {first:?}");
}

#[sqlx::test]
async fn a_moving_unknown_appointment_is_not_found(pool: PgPool) {
    let s = salon(pool).await;
    let request = s.request(&s.ann, &s.haircut, 9, 10);
    let slot = s.slot(&request).await;
    assert!(matches!(Reservations::new(s.pool.clone()).reschedule(AppointmentId(Uuid::new_v4()), &request, &slot).await, Err(Error::NotFound)));
}
