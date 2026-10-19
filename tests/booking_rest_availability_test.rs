//! Specification: `POST /api/v1/orgs/{org_id}/availability:search`, a custom method (AIP-136) whose body is a search
//! request (parties with their services, a window of instants, `party_offset` and `service_gap` as ISO 8601 durations)
//! and whose answer is the slots the engine finds, with the next day that has one when there are none.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, BookableRow, ServiceRow};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

struct Salon {
    app: axum::Router,
    org: Uuid,
    room: BookableRow,
    stylist: BookableRow,
    haircut: ServiceRow,
}

/// New York, a room and a stylist open on Wednesday 2026-11-04 from 9:00 to 17:00 local time (14:00 to 22:00 UTC).
async fn salon(pool: PgPool) -> Salon {
    let org = clinic(&pool).await;
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(&pool, org, "stylist", 1, &[("role", "stylist"), ("gender", "female")]).await;
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let app = booking_router(pool);
    for bookable in [&room, &stylist] {
        rest_open(&app, org, &json!({"id": bookable.id})).await;
    }
    Salon { app, org, room, stylist, haircut }
}

fn search(salon: &Salon, from: &str, to: &str) -> Value {
    search_for(None, &json!({"id": salon.haircut.id}), from, to)
}

async fn found(salon: &Salon, body: Value) -> (StatusCode, Value) {
    call(&salon.app, "POST", &format!("/api/v1/orgs/{}/availability:search", salon.org), Some(body)).await
}

#[sqlx::test]
async fn slots_are_found_with_their_placements_allocations_and_price(pool: PgPool) {
    let salon = salon(pool).await;
    let (status, answer) = found(&salon, search(&salon, "14:00", "16:00")).await;
    assert_eq!(status, StatusCode::OK, "{answer}");
    let slots = answer["slots"].as_array().unwrap();
    assert_eq!(slots.len(), 5);
    let first = &slots[0];
    assert_eq!(first["price"], json!({"micros": 100_000_000, "currency": "USD"}));
    assert_eq!(first["placements"][0]["service"], json!(salon.haircut.id));
    assert_eq!(first["placements"][0]["during"], json!({"start": "2026-11-04T14:00:00Z", "end": "2026-11-04T15:00:00Z"}));
    let allocated: Vec<_> = first["allocations"].as_array().unwrap().iter().map(|a| a["bookable"].clone()).collect();
    assert_eq!(allocated.len(), 2);
    assert!(allocated.contains(&json!(salon.room.id)) && allocated.contains(&json!(salon.stylist.id)));
    assert_eq!(first["allocations"][0]["engagement"], json!("engaged"));
    assert!(answer["next_available_day"].is_null());
}

#[sqlx::test]
async fn an_instant_may_be_written_with_any_offset(pool: PgPool) {
    let salon = salon(pool).await;
    let (_, utc) = found(&salon, search(&salon, "14:00", "16:00")).await;
    let mut body = search(&salon, "14:00", "16:00");
    body["within"] = json!({"start": "2026-11-04T09:00:00-05:00", "end": "2026-11-04T11:00:00-05:00"});
    assert_eq!(found(&salon, body).await.1, utc);
}

#[sqlx::test]
async fn limit_caps_the_slots(pool: PgPool) {
    let salon = salon(pool).await;
    let mut body = search(&salon, "14:00", "22:00");
    body["limit"] = json!(2);
    assert_eq!(found(&salon, body).await.1["slots"].as_array().unwrap().len(), 2);
}

#[sqlx::test]
async fn a_want_is_an_aip_160_filter_on_the_chosen_bookables(pool: PgPool) {
    let salon = salon(pool).await;
    let mut body = search(&salon, "14:00", "16:00");
    body["parties"][0]["services"][0]["wants"] = json!([{"Filter": "gender = \"female\""}]);
    assert!(!found(&salon, body.clone()).await.1["slots"].as_array().unwrap().is_empty());
    body["parties"][0]["services"][0]["wants"] = json!([{"Filter": "gender = \"male\""}]);
    assert!(found(&salon, body.clone()).await.1["slots"].as_array().unwrap().is_empty());
    body["parties"][0]["services"][0]["wants"] = json!([{"Filter": "gender ="}]);
    assert_eq!(found(&salon, body).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn with_nothing_open_the_next_day_with_a_slot_is_given(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    for bookable in [&salon.room, &salon.stylist] {
        let recurrence = "DTSTART;TZID=America/New_York:20261111T100000\nRRULE:FREQ=DAILY;COUNT=1";
        let body = json!({"org_id": salon.org, "service_id": null, "bookable_id": bookable.id, "kind": "open", "recurrence": recurrence, "span": "PT2H", "reason": null, "notes": null});
        created(&salon.app, salon.org, "schedules", body).await;
    }
    let mut body = search(&salon, "14:00", "22:00");
    body["within"] = json!({"start": "2026-11-05T14:00:00Z", "end": "2026-11-05T22:00:00Z"});
    let (_, answer) = found(&salon, body.clone()).await;
    assert_eq!((answer["slots"].clone(), answer["next_available_day"].clone()), (json!([]), json!("2026-11-11")));
    body["horizon_days"] = json!(3);
    assert!(found(&salon, body).await.1["next_available_day"].is_null());
}

#[sqlx::test]
async fn parties_may_start_apart_by_the_party_offset(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let second = bookable(&pool, salon.org, "second stylist", 1, &[("role", "stylist")]).await;
    let suite = bookable(&pool, salon.org, "suite", 2, &[("kind", "room")]).await;
    for bookable in [&second, &suite] {
        rest_open(&salon.app, salon.org, &json!({"id": bookable.id})).await;
    }
    let party = json!({"client": null, "services": [{"service": salon.haircut.id}]});
    let mut body = json!({"parties": [party.clone(), party], "within": {"start": "2026-11-04T14:00:00Z", "end": "2026-11-04T15:30:00Z"}});
    let starts_apart = |answer: &Value| answer["slots"].as_array().unwrap().iter().any(|slot| slot["placements"][0]["during"] != slot["placements"][1]["during"]);
    assert!(!starts_apart(&found(&salon, body.clone()).await.1));
    body["party_offset"] = json!("PT30M");
    assert!(starts_apart(&found(&salon, body).await.1));
}

#[sqlx::test]
async fn a_bad_request_is_an_aip_193_error(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let elsewhere = org(&pool, "elsewhere").await;
    let foreign = service(&pool, elsewhere, "foreign", 30, None, is("kind", "room")).await;
    let service_id = salon.haircut.id;
    let window = json!({"start": "2026-11-04T14:00:00Z", "end": "2026-11-04T16:00:00Z"});
    let one = |party: Value| json!({"parties": [party], "within": window});
    let party = |service: Uuid| json!({"services": [{"service": service}]});
    let cases = [
        (json!({"parties": [], "within": window}), StatusCode::BAD_REQUEST),
        (one(json!({"services": []})), StatusCode::BAD_REQUEST),
        (json!({"parties": [party(service_id.0)], "within": {"start": "2026-11-04T16:00:00Z", "end": "2026-11-04T14:00:00Z"}}), StatusCode::BAD_REQUEST),
        (json!({"parties": [party(service_id.0)], "within": {"start": "2026-11-01T00:00:00Z", "end": "2026-12-25T00:00:00Z"}}), StatusCode::BAD_REQUEST),
        (json!({"parties": [party(service_id.0)], "within": {"start": "tomorrow", "end": "2026-11-04T16:00:00Z"}}), StatusCode::BAD_REQUEST),
        (json!({"parties": [party(service_id.0)], "within": window, "party_offset": "soon"}), StatusCode::BAD_REQUEST),
        (one(party(Uuid::new_v4())), StatusCode::NOT_FOUND),
        (one(party(foreign.id.0)), StatusCode::NOT_FOUND),
        (one(json!({"client": Uuid::new_v4(), "services": [{"service": service_id}]})), StatusCode::NOT_FOUND),
    ];
    for (body, expected) in cases {
        let (status, error) = found(&salon, body.clone()).await;
        assert_eq!(status, expected, "{body}");
        assert_eq!(error["error"]["code"], json!(expected.as_u16()), "{body}");
    }
    assert_eq!(call(&salon.app, "POST", &format!("/api/v1/orgs/{}/availability:search", Uuid::new_v4()), Some(search(&salon, "14:00", "16:00"))).await.0, StatusCode::NOT_FOUND);
}
