//! Specification: docs/TPD-booking.md, D7 and the user's answer of 2026-10-18: when a client id is given, the times the
//! client already holds in the system (a live appointment, or a hold that has not expired) are left out of the search,
//! and a hold over them is refused. A cancelled appointment frees the time.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{
    Appointments, BookableRow, ClientRow, Confirmation, Error, IdempotencyKey, Party, Reservations, Search, SearchRequest, ServiceRow, ServiceWant, Slot, Slots,
};
use sqlx::PgPool;

struct World {
    pool: PgPool,
    haircut: ServiceRow,
    ann: ClientRow,
    ben: ClientRow,
    _room: BookableRow,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "salon").await;
    let room = bookable(&pool, org, "room", 3, &[("kind", "room")]).await;
    for name in ["one", "two", "three"] {
        let stylist = bookable(&pool, org, name, 1, &[("role", "stylist")]).await;
        open_rule(&pool, Where { bookable: Some(stylist.id), ..Where::default() }, 9, 8).await;
    }
    open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let (ann, ben) = (client(&pool).await, client(&pool).await);
    World { pool, haircut, ann, ben, _room: room }
}

impl World {
    fn request_for(&self, client: Option<&ClientRow>, from_hour: u32, to_hour: u32) -> SearchRequest {
        let mut request = request(self.haircut.id, between(1, from_hour, to_hour));
        request.parties = vec![Party { client: client.map(|c| c.id), seats: 1, services: vec![ServiceWant { service: self.haircut.id, wants: vec![] }] }];
        request
    }

    async fn find(&self, client: Option<&ClientRow>, from_hour: u32, to_hour: u32) -> Vec<Slot> {
        Slots::new(self.pool.clone()).find(&self.request_for(client, from_hour, to_hour)).await.unwrap()
    }

    async fn starts(&self, client: Option<&ClientRow>) -> Vec<chrono::DateTime<Utc>> {
        let slots = self.find(client, 9, 12).await;
        slots.iter().map(|slot| slot.placements[0].during.start).collect()
    }

    /// The client books the first hour from 9:00.
    async fn book(&self, client: &ClientRow, key: &str) -> megh::booking::AppointmentRow {
        let request = self.request_for(Some(client), 9, 10);
        let slot = Slots::new(self.pool.clone()).find(&request).await.unwrap().remove(0);
        let reservations = Reservations::new(self.pool.clone());
        let hold = reservations.hold(&request, &slot, Utc::now() + Duration::minutes(10)).await.unwrap();
        let confirmation = Confirmation { clients: vec![client.id], payer: client.id, payments: vec![], attributes: serde_json::json!({}) };
        reservations.confirm(hold.id, confirmation, IdempotencyKey(key.into())).await.unwrap()
    }
}

#[sqlx::test]
async fn the_times_a_client_is_booked_are_left_out_of_their_search(pool: PgPool) {
    let w = world(pool).await;
    assert_eq!(w.starts(Some(&w.ann)).await.first(), Some(&at(1, 9, 0)));
    w.book(&w.ann, "ann-9").await;
    let starts = w.starts(Some(&w.ann)).await;
    assert_eq!(starts.first(), Some(&at(1, 10, 0)));
    assert!(starts.iter().all(|start| *start >= at(1, 10, 0)));
}

#[sqlx::test]
async fn other_clients_and_a_search_without_a_client_are_not_affected(pool: PgPool) {
    let w = world(pool).await;
    w.book(&w.ann, "ann-9").await;
    assert_eq!(w.starts(Some(&w.ben)).await.first(), Some(&at(1, 9, 0)));
    assert_eq!(w.starts(None).await.first(), Some(&at(1, 9, 0)));
}

#[sqlx::test]
async fn a_hold_blocks_the_clients_time_until_it_expires(pool: PgPool) {
    let w = world(pool.clone()).await;
    let request = w.request_for(Some(&w.ann), 9, 10);
    let slot = Slots::new(pool.clone()).find(&request).await.unwrap().remove(0);
    let hold = Reservations::new(pool.clone()).hold(&request, &slot, Utc::now() + Duration::minutes(10)).await.unwrap();
    assert_eq!(w.starts(Some(&w.ann)).await.first(), Some(&at(1, 10, 0)));
    sqlx::query("UPDATE holds SET expires_at = now() - interval '1 minute' WHERE id = $1").bind(hold.id).execute(&pool).await.unwrap();
    assert_eq!(w.starts(Some(&w.ann)).await.first(), Some(&at(1, 9, 0)));
}

#[sqlx::test]
async fn cancelling_frees_the_clients_time(pool: PgPool) {
    let w = world(pool.clone()).await;
    let appointment = w.book(&w.ann, "ann-9").await;
    Reservations::new(pool).cancel(appointment.id).await.unwrap();
    assert_eq!(w.starts(Some(&w.ann)).await.first(), Some(&at(1, 9, 0)));
}

#[sqlx::test]
async fn a_hold_over_a_time_the_client_already_has_is_refused(pool: PgPool) {
    let w = world(pool.clone()).await;
    let slot_without_client = Slots::new(pool.clone()).find(&w.request_for(None, 9, 10)).await.unwrap().remove(0);
    w.book(&w.ann, "ann-9").await;
    let result = Reservations::new(pool).hold(&w.request_for(Some(&w.ann), 9, 10), &slot_without_client, Utc::now() + Duration::minutes(10)).await;
    assert!(matches!(result, Err(Error::ClientBusy(client)) if client == w.ann.id));
}

#[sqlx::test]
async fn each_service_of_a_party_keeps_clear_of_the_clients_time(pool: PgPool) {
    let w = world(pool.clone()).await;
    let colour = service(&pool, w.haircut.org_id, "colour", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    open_rule(&pool, Where { service: Some(colour.id), ..Where::default() }, 9, 8).await;
    w.book(&w.ann, "ann-9").await;
    let mut request = w.request_for(Some(&w.ann), 8, 13);
    request.parties[0].services = vec![ServiceWant { service: colour.id, wants: vec![] }, ServiceWant { service: w.haircut.id, wants: vec![] }];
    let slots = Slots::new(pool).find(&request).await.unwrap();
    assert!(!slots.is_empty());
    for slot in &slots {
        assert!(slot.placements.iter().all(|placement| placement.during.start >= at(1, 10, 0)), "{slot:?}");
    }
}
