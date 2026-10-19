//! Specification: the user's request of 2026-10-18: an appointment is rescheduled by a `PATCH` with a new slot.
//! Same id, payer and payments; the old time is freed; the move may overlap the appointment's own old time.

mod booking_support;

use axum::http::StatusCode;
use axum::Router;
use booking_support::rest::*;
use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{booking_router, Appointments, AppointmentId, Error, Party, Reservations, Search, ServiceWant, Slots};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

struct Salon {
    app: Router,
    org: Uuid,
    haircut: Value,
    colour: Value,
    ann: Value,
    ben: Value,
}

/// A stylist, a chair and two services open on Wednesday 2026-11-04 from 9:00 to 17:00 (New York).
async fn salon(pool: PgPool) -> Salon {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let (chair, stylist) = (created(&app, org, "bookables", json!({"org_id": org, "name": "Chair", "capacity": 1, "attributes": {"kind": "chair"}})).await, created(&app, org, "bookables", json!({"org_id": org, "name": "Mia", "capacity": 1, "attributes": {"role": "stylist"}})).await);
    for bookable in [&chair, &stylist] {
        created(&app, org, "schedules", json!({"bookable_id": bookable["id"], "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"})).await;
    }
    let steps = |minutes: u32| json!([{"duration": format!("{}s", minutes * 60), "needs": [{"who": "role = \"stylist\""}, {"who": "kind = \"chair\""}]}]);
    let price = |units: &str| json!({"currency_code": "USD", "units": units, "nanos": 0});
    let haircut = created(&app, org, "services", json!({"display_name": "Haircut", "price": price("40"), "steps": steps(60)})).await;
    let colour = created(&app, org, "services", json!({"display_name": "Colour", "price": price("90"), "steps": steps(60)})).await;
    let (ann, ben) = (created(&app, org, "clients", json!({"org_id": org, "name": "Ann", "first_name": "Ann", "last_name": "Client", "attributes": {}})).await, created(&app, org, "clients", json!({"org_id": org, "name": "Ben", "first_name": "Ben", "last_name": "Client", "attributes": {}})).await);
    Salon { app, org, haircut, colour, ann, ben }
}

impl Salon {
    fn uri(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.org)
    }

    async fn slots(&self, service: &Value, client: &Value, from: &str, to: &str, moving: Option<&Value>) -> Vec<Value> {
        let mut body = json!({"parties": [{"client_id": client["id"], "services": [{"service_id": service["id"]}]}], "start_time": from, "end_time": to});
        if let Some(appointment) = moving {
            body["reschedule"] = appointment["id"].clone();
        }
        call(&self.app, "POST", &self.uri("availability:search"), Some(body)).await.1["slots"].as_array().unwrap().clone()
    }

    async fn book(&self, service: &Value, client: &Value, hour: u32, key: &str) -> Value {
        let slot = self.slots(service, client, &format!("2026-11-04T{hour:02}:00:00"), &format!("2026-11-04T{:02}:00:00", hour + 1), None).await.remove(0);
        let body = json!({"state": "booked", "slot": slot, "request_id": key, "payer_id": client["id"]});
        let (status, appointment) = call(&self.app, "POST", &self.uri("appointments"), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{appointment}");
        appointment
    }

    async fn move_to(&self, appointment: &Value, slot: &Value) -> (StatusCode, Value) {
        call(&self.app, "PATCH", &path(self.org, "appointments", appointment), Some(json!({"slot": slot}))).await
    }
}

#[sqlx::test]
async fn an_appointment_moves_to_another_time_keeping_its_id_payer_and_lines(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    let later = s.slots(&s.haircut, &s.ann, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    let (status, moved) = s.move_to(&booked, &later).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!((moved["id"].clone(), moved["state"].clone(), moved["payer_id"].clone()), (booked["id"].clone(), json!("booked"), booked["payer_id"].clone()));
    assert_eq!((moved["start_time"].clone(), moved["end_time"].clone()), (json!("2026-11-04T11:00:00-05:00"), json!("2026-11-04T12:00:00-05:00")));
    assert_eq!(moved["lines"][0]["client_id"], booked["lines"][0]["client_id"]);
    assert_eq!(moved["allocations"].as_array().unwrap().len(), 2);
    assert_eq!(call(&s.app, "GET", &path(s.org, "appointments", &booked), None).await.1["start_time"], moved["start_time"]);
}

#[sqlx::test]
async fn the_old_time_is_free_again_and_the_new_one_is_taken(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    let later = s.slots(&s.haircut, &s.ann, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    assert_eq!(s.move_to(&booked, &later).await.0, StatusCode::OK);
    assert_eq!(s.slots(&s.haircut, &s.ben, "2026-11-04T09:00:00", "2026-11-04T10:00:00", None).await.len(), 1);
    assert!(s.slots(&s.haircut, &s.ben, "2026-11-04T11:00:00", "2026-11-04T12:00:00", None).await.is_empty());
}

#[sqlx::test]
async fn a_move_may_overlap_the_appointments_own_old_time(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    assert!(s.slots(&s.haircut, &s.ann, "2026-11-04T09:30:00", "2026-11-04T10:30:00", None).await.is_empty(), "an ordinary search sees the old time as taken");
    let half_past = s.slots(&s.haircut, &s.ann, "2026-11-04T09:30:00", "2026-11-04T10:30:00", Some(&booked)).await.remove(0);
    let (status, moved) = s.move_to(&booked, &half_past).await;
    assert_eq!((status, moved["start_time"].clone()), (StatusCode::OK, json!("2026-11-04T09:30:00-05:00")));
}

#[sqlx::test]
async fn a_slot_that_was_taken_in_the_meantime_is_refused_and_nothing_moves(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    let eleven = s.slots(&s.haircut, &s.ann, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    s.book(&s.haircut, &s.ben, 11, "ben").await;
    let (status, error) = s.move_to(&booked, &eleven).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")));
    assert_eq!(call(&s.app, "GET", &path(s.org, "appointments", &booked), None).await.1["start_time"], json!("2026-11-04T09:00:00-05:00"));
}

#[sqlx::test]
async fn the_slot_must_be_for_the_same_services_and_clients(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    let other_service = s.slots(&s.colour, &s.ann, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    let other_client = s.slots(&s.haircut, &s.ben, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    for slot in [other_service, other_client] {
        let (status, error) = s.move_to(&booked, &slot).await;
        assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")), "{slot}");
    }
}

#[sqlx::test]
async fn a_cancelled_appointment_cannot_be_moved(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    let later = s.slots(&s.haircut, &s.ann, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    assert_eq!(call(&s.app, "DELETE", &path(s.org, "appointments", &booked), None).await.0, StatusCode::OK);
    assert_eq!(s.move_to(&booked, &later).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_held_appointment_moves_and_keeps_its_expiry(pool: PgPool) {
    let s = salon(pool).await;
    let slot = s.slots(&s.haircut, &s.ann, "2026-11-04T09:00:00", "2026-11-04T10:00:00", None).await.remove(0);
    let (_, held) = call(&s.app, "POST", &s.uri("appointments"), Some(json!({"state": "held", "slot": slot}))).await;
    let later = s.slots(&s.haircut, &s.ann, "2026-11-04T13:00:00", "2026-11-04T14:00:00", Some(&held)).await.remove(0);
    let (status, moved) = s.move_to(&held, &later).await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!((moved["state"].clone(), moved["start_time"].clone(), moved["expire_time"].clone()), (json!("held"), json!("2026-11-04T13:00:00-05:00"), held["expire_time"].clone()));
    assert_eq!(s.slots(&s.haircut, &s.ben, "2026-11-04T09:00:00", "2026-11-04T10:00:00", None).await.len(), 1);
}

#[sqlx::test]
async fn the_client_cannot_be_moved_onto_a_time_they_already_have(pool: PgPool) {
    let s = salon(pool).await;
    let first = s.book(&s.haircut, &s.ann, 9, "first").await;
    let noon = s.slots(&s.colour, &s.ann, "2026-11-04T12:00:00", "2026-11-04T13:00:00", None).await.remove(0);
    let (_, second) = call(&s.app, "POST", &s.uri("appointments"), Some(json!({"state": "booked", "slot": noon, "request_id": "second", "payer_id": s.ann["id"]}))).await;
    let nine = s.slots(&s.haircut, &s.ben, "2026-11-04T09:00:00", "2026-11-04T10:00:00", None).await;
    assert!(nine.is_empty());
    let into_noon = s.slots(&s.haircut, &s.ann, "2026-11-04T12:00:00", "2026-11-04T13:00:00", Some(&first)).await;
    assert!(into_noon.is_empty(), "noon is taken by the stylist's colour for the same client; {second}");
}

#[sqlx::test]
async fn only_the_slot_can_be_updated(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.haircut, &s.ann, 9, "ann").await;
    let later = s.slots(&s.haircut, &s.ann, "2026-11-04T11:00:00", "2026-11-04T12:00:00", Some(&booked)).await.remove(0);
    let uri = path(s.org, "appointments", &booked);
    assert_eq!(call(&s.app, "PATCH", &format!("{uri}?update_mask=payer_id"), Some(json!({"slot": later}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&s.app, "PATCH", &uri, Some(json!({"slot": later, "payer_id": s.ben["id"]}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&s.app, "PATCH", &uri, Some(json!({}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&s.app, "PATCH", &format!("{uri}?update_mask=slot"), Some(json!({"slot": later}))).await.0, StatusCode::OK);
}

#[sqlx::test]
async fn moving_onto_a_time_that_is_full_is_occupied(pool: PgPool) {
    let org = org(&pool, "salon").await;
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(&pool, org, "stylist", 2, &[("role", "stylist")]).await;
    for found in [&room, &stylist] {
        open_rule(&pool, Where { bookable: Some(found.id), ..Where::default() }, 9, 8).await;
    }
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let request_at = |hour: u32| {
        let mut request = request(haircut.id, between(1, hour, hour + 1));
        request.parties = vec![Party { client: None, seats: 1, services: vec![ServiceWant { service: haircut.id, wants: vec![] }] }];
        request
    };
    let (reservations, slots) = (Reservations::new(pool.clone()), Slots::new(pool.clone()));
    let first = request_at(9);
    let nine = slots.find(&first).await.unwrap().remove(0);
    let held = reservations.hold(&first, &nine, Utc::now() + Duration::minutes(5)).await.unwrap();
    let ten_request = request_at(10);
    let ten = slots.find(&ten_request).await.unwrap().remove(0);
    reservations.hold(&ten_request, &ten, Utc::now() + Duration::minutes(5)).await.unwrap();
    let result = reservations.reschedule(AppointmentId(held.id.0), &ten_request, &ten).await;
    assert!(matches!(result, Err(Error::Occupied(id)) if id == room.id), "{result:?}");
}
