//! Specification: appointments over REST under `/api/v1/orgs/{org_id}/appointments`. A slot found by a search is booked as
//! one appointment for each of its placements, directly or by holding it first; the answers are the entities; the
//! methods of an appointment follow Google AIP-136 (`:hold`, `:book`, `:confirm`); a retry is safe with a `request_id`.

mod booking_support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{booking_router, ClientRow, PaymentMethodRow, ServiceRow};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri).header("sec-fetch-site", "same-origin");
    let request = match body {
        Some(body) => request.header("content-type", "application/json").body(Body::from(body.to_string())),
        None => request.body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap_or(Value::Null))
}

struct Salon {
    app: Router,
    org: Uuid,
    haircut: ServiceRow,
    client: ClientRow,
    on_spot: PaymentMethodRow,
}

/// New York, a room and a stylist open on Wednesday 2026-11-04 from 9:00 to 17:00 local time (UTC-5), a haircut of an hour.
async fn salon(pool: PgPool) -> Salon {
    let org = org(&pool, "salon").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(&pool).await.unwrap();
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await;
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let (client, on_spot) = (client_of(&pool, org).await, on_spot(&pool, org).await);
    let app = booking_router(pool.clone());
    for bookable in [&room, &stylist] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/schedules"), Some(body)).await.0, StatusCode::OK);
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

    async fn slots(&self, from: &str, to: &str) -> Vec<Value> {
        let body = json!({"parties": [{"client_id": self.client.id, "services": [{"service_id": self.haircut.id}]}], "start_time": from, "end_time": to});
        let (status, found) = call(&self.app, "POST", &self.uri("availability:search"), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        found["slots"].as_array().unwrap().clone()
    }

    async fn first_slot(&self) -> Value {
        self.slots("2026-11-04T09:00:00", "2026-11-04T12:00:00").await.remove(0)
    }

    async fn book(&self, slot: &Value, request_id: &str) -> Value {
        let body = json!({"slot": slot, "payments": [self.payment()], "request_id": request_id});
        let (status, booked) = call(&self.app, "POST", &self.uri("appointments"), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{booked}");
        booked["data"][0].clone()
    }

    async fn hold(&self, slot: &Value) -> Value {
        let until = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let (status, held) = call(&self.app, "POST", &self.uri("appointments:hold"), Some(json!({"slot": slot, "expires_at": until}))).await;
        assert_eq!(status, StatusCode::OK, "{held}");
        held["data"][0].clone()
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
    let slot = s.first_slot().await;
    s.book(&slot, "request-1").await;
    let starts: Vec<Value> = s.slots("2026-11-04T09:00:00", "2026-11-04T12:00:00").await.iter().map(|slot| slot["start_time"].clone()).collect();
    assert!(!starts.contains(&slot["start_time"]));
}

#[sqlx::test]
async fn the_same_request_id_returns_the_same_appointment_and_another_request_is_refused(pool: PgPool) {
    let s = salon(pool).await;
    let slot = s.first_slot().await;
    let (first, again) = (s.book(&slot, "same").await, s.book(&slot, "same").await);
    assert_eq!(first["id"], again["id"]);
    let other = json!({"slot": s.slots("2026-11-04T10:00:00", "2026-11-04T12:00:00").await.remove(0), "payments": [s.payment()], "request_id": "same"});
    let (status, error) = call(&s.app, "POST", &s.uri("appointments"), Some(other)).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::CONFLICT, json!("ALREADY_EXISTS")));
}

#[sqlx::test]
async fn a_held_appointment_blocks_the_slot_and_is_booked_under_the_same_id(pool: PgPool) {
    let s = salon(pool).await;
    let slot = s.first_slot().await;
    let held = s.hold(&slot).await;
    assert_eq!((held["status"].clone(), held["payment"].clone()), (json!("held"), Value::Null));
    assert!(held["expires_at"].is_string());
    let offered: Vec<Value> = s.slots("2026-11-04T09:00:00", "2026-11-04T12:00:00").await.iter().map(|candidate| candidate["start_time"].clone()).collect();
    assert!(!offered.contains(&slot["start_time"]));
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
    let slot = s.first_slot().await;
    let booked = s.book(&slot, "request-1").await;
    let (status, cancelled) = call(&s.app, "DELETE", &s.path(&booked), None).await;
    assert_eq!((status, cancelled["status"].clone()), (StatusCode::OK, json!("cancelled")));
    assert!(cancelled["deleted_at"].is_string());
    assert_eq!(call(&s.app, "DELETE", &s.path(&booked), None).await.0, StatusCode::NOT_FOUND);
    let offered: Vec<Value> = s.slots("2026-11-04T09:00:00", "2026-11-04T12:00:00").await.iter().map(|candidate| candidate["start_time"].clone()).collect();
    assert!(offered.contains(&slot["start_time"]));
}

#[sqlx::test]
async fn an_appointment_is_moved_to_another_slot(pool: PgPool) {
    let s = salon(pool).await;
    let booked = s.book(&s.first_slot().await, "request-1").await;
    let later = s.slots("2026-11-04T13:00:00", "2026-11-04T15:00:00").await.remove(0);
    let (status, moved) = call(&s.app, "PATCH", &s.path(&booked), Some(json!({"slot": later}))).await;
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
    let mut slot = s.first_slot().await;
    slot["allocations"][0]["start_time"] = json!("2026-11-04T09:15:00");
    let (status, error) = call(&s.app, "POST", &s.uri("appointments"), Some(json!({"slot": slot, "payments": [s.payment()], "request_id": "r"}))).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")));
}
