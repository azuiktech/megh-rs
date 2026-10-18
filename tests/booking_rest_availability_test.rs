//! Specification: docs/TPD-booking.md, R4 and R6: `POST /api/v1/orgs/{org_id}/availability:search`, a custom method
//! (AIP-136) taking parties, a window in the org's local time, `party_offset` and `service_gap` as AIP-142 durations.

mod booking_support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use booking_support::*;
use megh::booking::{booking_router, BookableRow, ServiceRow};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(app: &Router, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::post(uri).header("content-type", "application/json").header("sec-fetch-site", "same-origin").body(Body::from(body.to_string())).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap_or(Value::Null))
}

struct Salon {
    app: Router,
    org: Uuid,
    room: BookableRow,
    stylist: BookableRow,
    haircut: ServiceRow,
}

/// New York, a room and a stylist open on Wednesday 2026-11-04 from 9:00 to 17:00 local time (UTC-5), made over REST.
async fn salon(pool: PgPool) -> Salon {
    let org = org(&pool, "salon").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(&pool).await.unwrap();
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(&pool, org, "stylist", 1, &[("role", "stylist"), ("gender", "female")]).await;
    let haircut = service(&pool, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let app = booking_router(pool);
    for bookable in [&room, &stylist] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        let (status, _) = call(&app, &format!("/api/v1/orgs/{org}/schedules"), body).await;
        assert_eq!(status, StatusCode::OK);
    }
    Salon { app, org, room, stylist, haircut }
}

fn search(salon: &Salon, start: &str, end: &str) -> Value {
    json!({"parties": [{"services": [{"service_id": salon.haircut.id}]}], "start_time": start, "end_time": end})
}

fn uri(salon: &Salon) -> String {
    format!("/api/v1/orgs/{}/availability:search", salon.org)
}

#[sqlx::test]
async fn slots_are_found_in_local_time_with_their_parties_allocations_and_price(pool: PgPool) {
    let salon = salon(pool).await;
    let (status, found) = call(&salon.app, &uri(&salon), search(&salon, "2026-11-04T09:00:00", "2026-11-04T11:00:00")).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let slots = found["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 5);
    let first = &slots[0];
    assert_eq!((first["start_time"].clone(), first["end_time"].clone()), (json!("2026-11-04T09:00:00-05:00"), json!("2026-11-04T10:00:00-05:00")));
    assert_eq!(first["price"], json!({"currency_code": "USD", "units": "100", "nanos": 0}));
    assert_eq!(first["parties"][0]["services"][0]["service_id"], json!(salon.haircut.id));
    assert_eq!(first["parties"][0]["services"][0]["start_time"], json!("2026-11-04T09:00:00-05:00"));
    let allocated: Vec<_> = first["allocations"].as_array().unwrap().iter().map(|a| a["bookable_id"].clone()).collect();
    assert_eq!(allocated.len(), 2);
    assert!(allocated.contains(&json!(salon.room.id)) && allocated.contains(&json!(salon.stylist.id)));
    assert_eq!(first["allocations"][0]["engagement"], json!("engaged"));
    assert!(found["next_available_day"].is_null());
}

#[sqlx::test]
async fn a_time_with_an_offset_means_the_same_instant(pool: PgPool) {
    let salon = salon(pool).await;
    let (_, local) = call(&salon.app, &uri(&salon), search(&salon, "2026-11-04T09:00:00", "2026-11-04T11:00:00")).await;
    let (_, offset) = call(&salon.app, &uri(&salon), search(&salon, "2026-11-04T14:00:00Z", "2026-11-04T11:00:00-05:00")).await;
    assert_eq!(local, offset);
}

#[sqlx::test]
async fn page_size_caps_the_slots(pool: PgPool) {
    let salon = salon(pool).await;
    let mut body = search(&salon, "2026-11-04T09:00:00", "2026-11-04T17:00:00");
    body["page_size"] = json!(2);
    let (_, found) = call(&salon.app, &uri(&salon), body).await;
    assert_eq!(found["slots"].as_array().unwrap().len(), 2);
}

#[sqlx::test]
async fn a_want_is_an_aip_160_filter_on_the_chosen_bookables(pool: PgPool) {
    let salon = salon(pool).await;
    let mut body = search(&salon, "2026-11-04T09:00:00", "2026-11-04T11:00:00");
    body["parties"][0]["services"][0]["wants"] = json!(["gender = \"female\""]);
    assert!(!call(&salon.app, &uri(&salon), body.clone()).await.1["slots"].as_array().unwrap().is_empty());
    body["parties"][0]["services"][0]["wants"] = json!(["gender = \"male\""]);
    assert!(call(&salon.app, &uri(&salon), body.clone()).await.1["slots"].as_array().unwrap().is_empty());
    body["parties"][0]["services"][0]["wants"] = json!(["gender ="]);
    assert_eq!(call(&salon.app, &uri(&salon), body).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn with_nothing_open_the_next_day_with_a_slot_is_given(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    for bookable in [&salon.room, &salon.stylist] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-11", "start_time": "10:00", "end_time": "12:00"});
        assert_eq!(call(&salon.app, &format!("/api/v1/orgs/{}/schedules", salon.org), body).await.0, StatusCode::OK);
    }
    let mut body = search(&salon, "2026-11-05T09:00:00", "2026-11-05T17:00:00");
    let (_, found) = call(&salon.app, &uri(&salon), body.clone()).await;
    assert_eq!((found["slots"].clone(), found["next_available_day"].clone()), (json!([]), json!("2026-11-11")));
    body["horizon_days"] = json!(3);
    assert!(call(&salon.app, &uri(&salon), body).await.1["next_available_day"].is_null());
}

#[sqlx::test]
async fn parties_may_start_apart_by_the_party_offset(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let second = bookable(&pool, salon.org, "second stylist", 1, &[("role", "stylist")]).await;
    let suite = bookable(&pool, salon.org, "suite", 2, &[("kind", "room")]).await;
    for bookable in [&second, &suite] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        assert_eq!(call(&salon.app, &format!("/api/v1/orgs/{}/schedules", salon.org), body).await.0, StatusCode::OK);
    }
    let party = json!({"services": [{"service_id": salon.haircut.id}]});
    let mut body = json!({"parties": [party.clone(), party], "start_time": "2026-11-04T09:00:00", "end_time": "2026-11-04T10:30:00"});
    let starts_apart = |found: &Value| found["slots"].as_array().unwrap().iter().any(|slot| slot["parties"][0]["services"][0]["start_time"] != slot["parties"][1]["services"][0]["start_time"]);
    assert!(!starts_apart(&call(&salon.app, &uri(&salon), body.clone()).await.1));
    body["party_offset"] = json!("1800s");
    assert!(starts_apart(&call(&salon.app, &uri(&salon), body).await.1));
}

#[sqlx::test]
async fn a_bad_request_is_an_aip_193_error(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let elsewhere = org(&pool, "elsewhere").await;
    let foreign = service(&pool, "foreign", 30, None, is("kind", "room")).await;
    sqlx::query("UPDATE services SET org_id = $1 WHERE id = $2").bind(elsewhere).bind(foreign.id).execute(&pool).await.unwrap();
    let window = ("2026-11-04T09:00:00", "2026-11-04T11:00:00");
    let one = |party: Value| json!({"parties": [party], "start_time": window.0, "end_time": window.1});
    let cases = [
        (json!({"parties": [], "start_time": window.0, "end_time": window.1}), StatusCode::BAD_REQUEST),
        (one(json!({"services": []})), StatusCode::BAD_REQUEST),
        (one(json!({"seats": 0, "services": [{"service_id": salon.haircut.id}]})), StatusCode::BAD_REQUEST),
        (json!({"parties": [{"services": [{"service_id": salon.haircut.id}]}], "start_time": window.1, "end_time": window.0}), StatusCode::BAD_REQUEST),
        (json!({"parties": [{"services": [{"service_id": salon.haircut.id}]}], "start_time": "2026-11-01T00:00:00", "end_time": "2026-12-25T00:00:00"}), StatusCode::BAD_REQUEST),
        (json!({"parties": [{"services": [{"service_id": salon.haircut.id}]}], "start_time": "tomorrow", "end_time": window.1}), StatusCode::BAD_REQUEST),
        (json!({"parties": [{"services": [{"service_id": salon.haircut.id}]}], "start_time": window.0, "end_time": window.1, "party_offset": "soon"}), StatusCode::BAD_REQUEST),
        (json!({"parties": [{"services": [{"service_id": salon.haircut.id}]}], "start_time": window.0, "end_time": window.1, "nonsense": 1}), StatusCode::BAD_REQUEST),
        (one(json!({"services": [{"service_id": Uuid::new_v4()}]})), StatusCode::NOT_FOUND),
        (one(json!({"services": [{"service_id": foreign.id}]})), StatusCode::NOT_FOUND),
        (one(json!({"client_id": Uuid::new_v4(), "services": [{"service_id": salon.haircut.id}]})), StatusCode::NOT_FOUND),
    ];
    for (body, expected) in cases {
        let (status, error) = call(&salon.app, &uri(&salon), body.clone()).await;
        assert_eq!(status, expected, "{body}");
        assert_eq!(error["error"]["code"], json!(expected.as_u16()), "{body}");
    }
    assert_eq!(call(&salon.app, &format!("/api/v1/orgs/{}/availability:search", Uuid::new_v4()), search(&salon, window.0, window.1)).await.0, StatusCode::NOT_FOUND);
}
