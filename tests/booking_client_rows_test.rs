//! Specification: the client is an axis like a room or a provider. Booking, holding or waitlisting a client's time
//! stores a row for the client; the client is then offered no overlapping slot until the appointment is cancelled.
//! Client rows never use the capacity of a room.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{Appointments, AppointmentRow, ClientId, ClientRow, IdempotencyKey, Party, PaymentMethodRow, Reservations, Search, SearchRequest, ServiceRow, ServiceWant, Slot, Slots, Status};
use sqlx::PgPool;
use uuid::Uuid;

struct Hall {
    pool: PgPool,
    class: ServiceRow,
    on_spot: PaymentMethodRow,
}

async fn hall(pool: PgPool, capacity: u32, waitlist: u32) -> Hall {
    let org = org(&pool, "hall").await;
    let room = bookable_with_waitlist(&pool, org, "room", capacity, waitlist, &[("kind", "room")]).await;
    open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let class = service(&pool, org, "class", 60, None, is("kind", "room")).await;
    let on_spot = on_spot(&pool, org).await;
    Hall { pool, class, on_spot }
}

impl Hall {
    fn request(&self, client: &ClientRow) -> SearchRequest {
        let party = Party { client: Some(client.id), services: vec![ServiceWant { service: self.class.id, wants: vec![] }] };
        SearchRequest { org: None, parties: vec![party], within: whole_day(1), party_offset: std::time::Duration::ZERO, service_gap: std::time::Duration::ZERO, slot_interval: minutes(15), limit: None }
    }

    async fn slots(&self, client: &ClientRow) -> Vec<Slot> {
        Slots::new(self.pool.clone()).find(&self.request(client)).await.unwrap()
    }

    async fn at_nine(&self, client: &ClientRow) -> Option<Slot> {
        self.slots(client).await.into_iter().find(|slot| slot.allocations[0].during.start == at(1, 9, 0))
    }

    async fn book(&self, client: &ClientRow, slot: &Slot) -> AppointmentRow {
        let payment = pays(client, &self.on_spot);
        Reservations::new(self.pool.clone()).book(&self.request(client), slot, vec![payment], IdempotencyKey(Uuid::new_v4().to_string())).await.unwrap().remove(0)
    }

    async fn rows(&self, client: ClientId) -> Vec<(String, String)> {
        sqlx::query_as("SELECT status, engagement FROM occupancies WHERE client_id = $1 AND deleted_at IS NULL").bind(client).fetch_all(&self.pool).await.unwrap()
    }
}

#[sqlx::test]
async fn booking_stores_a_row_for_the_client_with_the_status_of_the_appointment(pool: PgPool) {
    let h = hall(pool, 1, 1).await;
    let ann = client(&h.pool).await;
    let slot = h.at_nine(&ann).await.unwrap();
    h.book(&ann, &slot).await;
    assert_eq!(h.rows(ann.id).await, vec![("booked".to_string(), "engaged".to_string())]);
}

#[sqlx::test]
async fn a_booked_client_is_offered_no_overlapping_slot_until_the_appointment_is_cancelled(pool: PgPool) {
    let h = hall(pool, 5, 0).await;
    let ann = client(&h.pool).await;
    let slot = h.at_nine(&ann).await.unwrap();
    let appointment = h.book(&ann, &slot).await;
    assert!(h.at_nine(&ann).await.is_none());
    Reservations::new(h.pool.clone()).transition(appointment.id, Status::Cancelled).await.unwrap();
    assert!(h.at_nine(&ann).await.is_some());
    assert!(h.rows(ann.id).await.is_empty());
}

#[sqlx::test]
async fn another_client_is_still_offered_the_time(pool: PgPool) {
    let h = hall(pool, 5, 0).await;
    let (ann, ben) = (client(&h.pool).await, client(&h.pool).await);
    let slot = h.at_nine(&ann).await.unwrap();
    h.book(&ann, &slot).await;
    assert!(h.at_nine(&ben).await.is_some());
}

#[sqlx::test]
async fn a_waitlisted_client_is_not_offered_an_overlapping_slot_either(pool: PgPool) {
    let h = hall(pool, 1, 1).await;
    let (ann, ben) = (client(&h.pool).await, client(&h.pool).await);
    let slot = h.at_nine(&ann).await.unwrap();
    h.book(&ann, &slot).await;
    let waiting = h.book(&ben, &h.at_nine(&ben).await.unwrap()).await;
    assert_eq!(waiting.status, Status::Waitlist);
    assert_eq!(h.rows(ben.id).await, vec![("waitlist".to_string(), "engaged".to_string())]);
    assert!(h.at_nine(&ben).await.is_none());
}

#[sqlx::test]
async fn a_hold_blocks_the_time_of_the_named_client(pool: PgPool) {
    let h = hall(pool, 5, 0).await;
    let ann = client(&h.pool).await;
    let slot = h.at_nine(&ann).await.unwrap();
    Reservations::new(h.pool.clone()).hold(&h.request(&ann), &slot, Utc::now() + Duration::minutes(10)).await.unwrap();
    assert!(h.at_nine(&ann).await.is_none());
}

#[sqlx::test]
async fn client_rows_use_no_capacity(pool: PgPool) {
    let h = hall(pool, 2, 0).await;
    let (ann, ben) = (client(&h.pool).await, client(&h.pool).await);
    h.book(&ann, &h.at_nine(&ann).await.unwrap()).await;
    let second = h.at_nine(&ben).await.expect("a second place");
    assert_eq!(h.book(&ben, &second).await.status, Status::Booked);
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM occupancies WHERE bookable_id IS NOT NULL AND deleted_at IS NULL").fetch_one(&h.pool).await.unwrap();
    assert_eq!(rows, 2);
}
