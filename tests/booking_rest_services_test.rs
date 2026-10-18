//! Specification: docs/TPD-booking.md, R5 and R6: services (steps, needs as AIP-160 filters, variants) over REST, in the
//! manner of Google AIP-131 to 135, 148, 160, 164 and 193, under `/api/v1/orgs/{org_id}/services`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn price(units: &str) -> Value {
    json!({"currency_code": "USD", "units": units, "nanos": 0})
}

fn colour() -> Value {
    json!({
        "display_name": "Hair colour",
        "price": price("120"),
        "steps": [
            {"duration": "600s", "needs": [{"who": "role = \"stylist\""}, {"who": "kind = \"chair\""}]},
            {"duration": "1800s", "needs": [{"who": "role = \"stylist\"", "engagement": "free"}, {"who": "kind = \"chair\""}]},
            {"duration": "1200s", "needs": [{"who": "role = \"stylist\""}, {"who": "kind = \"chair\""}]}
        ],
        "variants": [{"when": "level = \"master\"", "duration": {"add": "900s"}, "price": {"add": price("30")}}],
        "attributes": {"category": "hair"}
    })
}

#[sqlx::test]
async fn a_service_is_created_with_steps_needs_and_variants(pool: PgPool) {
    let org = clinic(&pool).await;
    let service = created(&booking_router(pool), org, "services", colour()).await;
    assert_eq!((service["display_name"].clone(), service["mode"].clone(), service["org_id"].clone()), (json!("Hair colour"), json!("appointment"), json!(org)));
    assert_eq!(service["price"], price("120"));
    let steps = service["steps"].as_array().unwrap();
    assert_eq!(steps.iter().map(|s| s["duration"].clone()).collect::<Vec<_>>(), vec![json!("600s"), json!("1800s"), json!("1200s")]);
    assert_eq!((steps[1]["needs"][0]["who"].clone(), steps[1]["needs"][0]["engagement"].clone(), steps[0]["client"].clone()), (json!("role = \"stylist\""), json!("free"), json!("present")));
    assert_eq!(service["variants"][0], json!({"when": "level = \"master\"", "duration": {"add": "900s"}, "price": {"add": price("30")}}));
    assert_eq!(service["name"], json!(format!("orgs/{org}/services/{}", service["id"].as_str().unwrap())));
}

#[sqlx::test]
async fn a_service_is_read_back_listed_and_filtered(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let hair = created(&app, org, "services", colour()).await;
    let menu = created(&app, org, "services", json!({"display_name": "Hair", "price": price("0"), "attributes": {"category": "menu"}})).await;
    assert_eq!(call(&app, "GET", &path(org, "services", &hair), None).await, (StatusCode::OK, hair.clone()));
    assert_eq!(menu["steps"], json!([]));
    assert_eq!(ids(&app, org, "services", "").await, vec![hair["id"].clone(), menu["id"].clone()]);
    assert_eq!(ids(&app, org, "services", &filter("attributes.category = \"menu\"")).await, vec![menu["id"].clone()]);
    assert_eq!(ids(&app, org, "services", &filter("display_name = \"Hair c*\"")).await, vec![hair["id"].clone()]);
}

#[sqlx::test]
async fn an_update_changes_the_masked_fields_and_keeps_the_rest(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let hair = created(&app, org, "services", colour()).await;
    let (_, repriced) = call(&app, "PATCH", &format!("{}?update_mask=price", path(org, "services", &hair)), Some(json!({"price": price("150"), "display_name": "ignored"}))).await;
    assert_eq!((repriced["price"].clone(), repriced["display_name"].clone(), repriced["steps"].clone(), repriced["variants"].clone()), (price("150"), hair["display_name"].clone(), hair["steps"].clone(), hair["variants"].clone()));
    let (_, shorter) = call(&app, "PATCH", &path(org, "services", &hair), Some(json!({"steps": [{"duration": "900s", "needs": [{"who": "kind = \"chair\""}]}]}))).await;
    assert_eq!(shorter["steps"].as_array().unwrap().len(), 1);
    assert_eq!((shorter["variants"].clone(), shorter["price"].clone()), (hair["variants"].clone(), price("150")));
}

#[sqlx::test]
async fn a_delete_keeps_the_service_with_its_delete_time(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let hair = created(&app, org, "services", colour()).await;
    let (status, deleted) = call(&app, "DELETE", &path(org, "services", &hair), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["delete_time"].is_string());
    assert!(ids(&app, org, "services", "").await.is_empty());
    assert_eq!(ids(&app, org, "services", "?show_deleted=true").await.len(), 1);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    let with_step = |step: Value| json!({"display_name": "S", "price": price("1"), "steps": [step]});
    let cases = [
        json!({"price": price("1")}),
        json!({"display_name": "S"}),
        json!({"display_name": "S", "price": {"currency_code": "USD", "units": "-5", "nanos": 0}}),
        json!({"display_name": "S", "price": price("1"), "mode": "teleport"}),
        json!({"display_name": "S", "price": price("1"), "unknown": 1}),
        json!({"display_name": "S", "price": price("1"), "org_id": other}),
        with_step(json!({"duration": "0s"})),
        with_step(json!({"duration": "soon"})),
        with_step(json!({"duration": "60s", "needs": [{"who": "role=\"stylist\""}]})),
        with_step(json!({"duration": "60s", "needs": [{"who": "role = \"stylist\"", "count": 0}]})),
        with_step(json!({"duration": "60s", "needs": [{"who": "role = \"stylist\"", "engagement": "sometimes"}]})),
        json!({"display_name": "S", "price": price("1"), "variants": [{"when": "level = ", "duration": {"add": "60s"}}]}),
    ];
    for body in cases {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/services"), Some(body.clone())).await;
        assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")), "{body}");
    }
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/services/{}", Uuid::new_v4()), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_services_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let body = |name: &str| json!({"display_name": name, "price": price("1")});
    let (own, below, elsewhere) = (created(&app, brand, "services", body("a")).await, created(&app, location, "services", body("b")).await, created(&app, other, "services", body("c")).await);
    assert_eq!(ids(&app, brand, "services", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "services", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "services", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_whole_salon_made_over_rest_is_found_by_the_search(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let chair = created(&app, org, "bookables", json!({"display_name": "Chair 1", "attributes": {"kind": "chair"}})).await;
    let master = created(&app, org, "bookables", json!({"display_name": "Mia", "attributes": {"role": "stylist", "level": "master"}})).await;
    let junior = created(&app, org, "bookables", json!({"display_name": "Jo", "attributes": {"role": "stylist", "level": "junior"}})).await;
    for bookable in [&chair, &master, &junior] {
        let body = json!({"bookable_id": bookable["id"], "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        created(&app, org, "schedules", body).await;
    }
    let service = created(&app, org, "services", colour()).await;
    let search = json!({"parties": [{"services": [{"service_id": service["id"]}]}], "start_time": "2026-11-04T09:00:00", "end_time": "2026-11-04T11:00:00"});
    let (status, found) = call(&app, "POST", &format!("/api/v1/orgs/{org}/availability:search"), Some(search)).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    let slots = found["slots"].as_array().unwrap();
    let by = |stylist: &Value| slots.iter().find(|slot| slot["allocations"].as_array().unwrap().iter().any(|a| a["bookable_id"] == stylist["id"])).unwrap().clone();
    let (with_master, with_junior) = (by(&master), by(&junior));
    let length = |slot: &Value| {
        let start = chrono::DateTime::parse_from_rfc3339(slot["start_time"].as_str().unwrap()).unwrap();
        let end = chrono::DateTime::parse_from_rfc3339(slot["end_time"].as_str().unwrap()).unwrap();
        (end - start).num_minutes()
    };
    assert_eq!((length(&with_master), length(&with_junior)), (75, 60));
    assert_eq!((with_master["price"].clone(), with_junior["price"].clone()), (price("150"), price("120")));
    let engagements: Vec<_> = with_junior["allocations"].as_array().unwrap().iter().filter(|a| a["bookable_id"] == junior["id"]).map(|a| a["engagement"].clone()).collect();
    assert_eq!(engagements, vec![json!("engaged"), json!("free"), json!("engaged")]);
}
