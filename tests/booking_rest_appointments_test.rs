//! Specification: appointments over REST under `/api/v1/orgs/{org_id}/appointments`. A slot found by a search is booked as
//! one appointment for each of its placements, directly or by holding it first; the body names the search request and
//! the slot it found, and the server uses its own equal slot; the answers are the entities; the methods of an appointment
//! follow Google AIP-136 (`:hold`, `:book`, `:confirm`); a retry is safe with a `request_id`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{booking_router, ClientRow, PaymentMethodRow, ServiceRow};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

struct Salon {
    app: axum::Router,
    org: Uuid,
    haircut: ServiceRow,
    client: ClientRow,
    on_spot: PaymentMethodRow,
}

/// New York, a room and a stylist open on Wednesday 2026-11-04 from 9:00 to 17:00 local time (UTC-5), a haircut of an hour.
async fn salon(pool: PgPool) -> Salon {
    let org = clinic(&pool).await;
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await;
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let (client, on_spot) = (client_of(&pool, org).await, on_spot(&pool, org).await);
    let app = booking_router(pool.clone());
    for bookable in [&room, &stylist] {
        rest_open(&app, org, &json!({"id": bookable.id})).await;
    }
    Salon { app, org, haircut, client, on_spot }
}

impl Salon {
    fn uri(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.org)
    }

    fn payment(&self) -> Value {
        json!({"payer_id": self.client.id, "method": self.on_spot.id, "amount": {"type": "uses", "value": 0}})
    }

    /// The request of a search for the client's haircut between two UTC times of 2026-11-04.
    fn request(&self, from: &str, to: &str) -> Value {
        search_for(Some(&json!({"id": self.client.id})), &json!({"id": self.haircut.id}), from, to)
    }

    async fn slots(&self, from: &str, to: &str) -> Vec<Value> {
        let (status, found) = call(&self.app, "POST", &self.uri("availability:search"), Some(self.request(from, to))).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        found["slots"].as_array().unwrap().clone()
    }

    /// The first slot of the morning, with the request that found it.
    async fn first_slot(&self) -> (Value, Value) {
        (self.request("14:00", "17:00"), self.slots("14:00", "17:00").await.remove(0))
    }

    async fn book(&self, found: &(Value, Value), request_id: &str) -> Value {
        let body = json!({"request": found.0, "slot": found.1, "payments": [self.payment()], "request_id": request_id});
        let (status, booked) = call(&self.app, "POST", &self.uri("appointments"), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{booked}");
        booked["data"][0].clone()
    }

    async fn hold(&self, found: &(Value, Value)) -> Value {
        let until = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let (status, held) = call(&self.app, "POST", &self.uri("appointments:hold"), Some(json!({"request": found.0, "slot": found.1, "expires_at": until}))).await;
        assert_eq!(status, StatusCode::OK, "{held}");
        held["data"][0].clone()
    }

    fn starts(slots: &[Value]) -> Vec<Value> {
        slots.iter().map(|slot| slot["placements"][0]["during"]["start"].clone()).collect()
    }

    fn path(&self, appointment: &Value) -> String {
        self.uri(&format!("appointments/{}", appointment["id"].as_str().unwrap()))
    }

    fn method(&self, appointment: &Value, verb: &str) -> String {
        self.uri(&format!("appointments/{}:{verb}", appointment["id"].as_str().unwrap()))
    }
}

#[sqlx::test]
async fn a_slot_is_booked_in_one_call_as_an_appointment_for_the_client(pool: PgPool) {
    let s = salon(pool).await;
    let appointment = s.book(&s.first_slot().await, "request-1").await;
    assert_eq!((appointment["status"].clone(), appointment["client_id"].clone(), appointment["service_id"].clone()), (json!("booked"), json!(s.client.id), json!(s.haircut.id)));
    assert_eq!((appointment["starts_at"].clone(), appointment["ends_at"].clone()), (json!("2026-11-04T14:00:00Z"), json!("2026-11-04T15:00:00Z")));
    assert_eq!(appointment["provider_ids"].as_array().unwrap().len(), 1);
    assert!(appointment["room_id"].is_string() && appointment["expires_at"].is_null() && appointment["deleted_at"].is_null());
    assert_eq!(appointment["payment"]["payer_id"], json!(s.client.id));
}

#[sqlx::test]
async fn a_booked_slot_is_no_longer_offered(pool: PgPool) {
    let s = salon(pool).await;
    let found = s.first_slot().await;
    s.book(&found, "request-1").await;
    assert!(!Salon::starts(&s.slots("14:00", "17:00").await).contains(&found.1["placements"][0]["during"]["start"]));
}

#[sqlx::test]
async fn the_same_request_id_returns_the_same_appointment_and_another_request_is_refused(pool: PgPool) {
    let s = salon(pool).await;
    let found = s.first_slot().await;
    let (first, again) = (s.book(&found, "same").await, s.book(&found, "same").await);
    assert_eq!(first["id"], again["id"]);
    let other = json!({"request": s.request("15:00", "17:00"), "slot": s.slots("15:00", "17:00").await.remove(0), "payments": [s.payment()], "request_id": "same"});
    let (status, error) = call(&s.app, "POST", &s.uri("appointments"), Some(other)).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::CONFLICT, json!("ALREADY_EXISTS")));
}

#[sqlx::test]
async fn a_held_appointment_blocks_the_slot_and_is_booked_under_the_same_id(pool: PgPool) {
    let s = salon(pool).await;
    let found = s.first_slot().await;
    let held = s.hold(&found).await;
    assert_eq!((held["status"].clone(), held["payment"].clone()), (json!("held"), Value::Null));
    assert!(held["expires_at"].is_string());
    assert!(!Salon::starts(&s.slots("14:00", "17:00").await).contains(&found.1["placements"][0]["during"]["start"]));
    let (status, booked) = call(&s.app, "POST", &s.method(&held, "book"), Some(json!({"payment": s.payment()}))).await;
    assert_eq!((status, booked["id"].clone(), booked["status"].clone()), (StatusCode::OK, held["id"].clone(), json!("booked")));
}

#[sqlx::test]
async fn an_expired_hold_is_gone(pool: PgPool) {
    let s = salon(pool.clone()).await;
    let held = s.hold(&s.first_slot().await).await;
    sqlx::query("UPDATE appointments SET expires_at = now() - interval '1 minute'").execute(&pool).await.unwrap();
    assert_eq!(call(&s.app, "GET", &s.path(&held), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_booked_appointment_is_confirmed_and_a_cancelled_one_cannot_be(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.first_slot().await, "request-1").await;
    let (status, confirmed) = call(&s.app, "POST", &s.method(&booked, "confirm"), Some(json!({}))).await;
    assert_eq!((status, confirmed["status"].clone()), (StatusCode::OK, json!("confirmed")));
    assert_eq!(call(&s.app, "DELETE", &s.path(&booked), None).await.0, StatusCode::OK);
    let (status, error) = call(&s.app, "POST", &s.method(&booked, "confirm"), Some(json!({}))).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("FAILED_PRECONDITION")));
}

#[sqlx::test]
async fn cancelling_keeps_the_appointment_as_cancelled_and_frees_the_time(pool: PgPool) {
    let s = salon(pool).await;
    let found = s.first_slot().await;
    let booked = s.book(&found, "request-1").await;
    let (status, cancelled) = call(&s.app, "DELETE", &s.path(&booked), None).await;
    assert_eq!((status, cancelled["status"].clone()), (StatusCode::OK, json!("cancelled")));
    assert!(cancelled["deleted_at"].is_string());
    assert_eq!(call(&s.app, "DELETE", &s.path(&booked), None).await.0, StatusCode::NOT_FOUND);
    assert!(Salon::starts(&s.slots("14:00", "17:00").await).contains(&found.1["placements"][0]["during"]["start"]));
}

#[sqlx::test]
async fn an_appointment_is_moved_to_another_slot(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.first_slot().await, "request-1").await;
    let (request, later) = (s.request("18:00", "20:00"), s.slots("18:00", "20:00").await.remove(0));
    let (status, moved) = call(&s.app, "PATCH", &s.path(&booked), Some(json!({"request": request, "slot": later}))).await;
    assert_eq!((status, moved["id"].clone(), moved["starts_at"].clone()), (StatusCode::OK, booked["id"].clone(), json!("2026-11-04T18:00:00Z")));
}

#[sqlx::test]
async fn appointments_are_read_back_and_listed_with_a_filter(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.first_slot().await, "request-1").await;
    assert_eq!(call(&s.app, "GET", &s.path(&booked), None).await, (StatusCode::OK, booked.clone()));
    let (_, all) = call(&s.app, "GET", &s.uri("appointments"), None).await;
    assert_eq!(all["data"], json!([booked]));
    let (_, waiting) = call(&s.app, "GET", &s.uri("appointments?filter=status%20%3D%20%22waitlist%22"), None).await;
    assert!(waiting["data"].as_array().unwrap().is_empty());
    let (_, mine) = call(&s.app, "GET", &s.uri(&format!("appointments?filter=client_id%20%3D%20%22{}%22", s.client.id.0)), None).await;
    assert_eq!(mine["data"].as_array().unwrap().len(), 1);
}

#[sqlx::test]
async fn a_slot_the_server_did_not_offer_is_refused(pool: PgPool) {
    let s = salon(pool).await;
    let (request, mut slot) = s.first_slot().await;
    slot["allocations"][0]["during"]["start"] = json!("2026-11-04T14:15:00Z");
    let (status, error) = call(&s.app, "POST", &s.uri("appointments"), Some(json!({"request": request, "slot": slot, "payments": [s.payment()], "request_id": "r"}))).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")));
}
