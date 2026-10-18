//! Specification: docs/TPD-booking.md, section 7 and R6: the waitlist over REST, under
//! `/api/v1/orgs/{org_id}/waitlistEntries`: join, leave, and the entries in the order they were requested.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

struct Studio {
    app: axum::Router,
    org: Uuid,
    class: Value,
    ann: Value,
    ben: Value,
}

async fn studio(pool: PgPool) -> Studio {
    let org = clinic(&pool).await;
    let app = booking_router(pool.clone());
    let class = created(&app, org, "services", json!({"display_name": "Music class", "mode": "class", "price": {"currency_code": "USD", "units": "20", "nanos": 0}})).await;
    let (ann, ben) = (created(&app, org, "clients", json!({"display_name": "Ann"})).await, created(&app, org, "clients", json!({"display_name": "Ben"})).await);
    Studio { app, org, class, ann, ben }
}

fn window(studio: &Studio, client: &Value, from: &str, to: &str) -> Value {
    json!({"client_id": client["id"], "target": {"window": {"service_id": studio.class["id"], "start_time": from, "end_time": to}}})
}

#[sqlx::test]
async fn a_client_joins_the_wait_for_a_service_in_a_window(pool: PgPool) {
    let studio = studio(pool).await;
    let entry = created(&studio.app, studio.org, "waitlistEntries", window(&studio, &studio.ann, "2026-11-05T18:00:00", "2026-11-05T19:00:00")).await;
    assert_eq!(entry["client_id"], studio.ann["id"]);
    assert_eq!(entry["target"], json!({"window": {"service_id": studio.class["id"], "start_time": "2026-11-05T18:00:00-05:00", "end_time": "2026-11-05T19:00:00-05:00"}}));
    assert!(entry["request_time"].is_string());
    assert_eq!(entry["name"], json!(format!("orgs/{}/waitlistEntries/{}", studio.org, entry["id"].as_str().unwrap())));
    assert_eq!(call(&studio.app, "GET", &path(studio.org, "waitlistEntries", &entry), None).await, (StatusCode::OK, entry));
}

#[sqlx::test]
async fn the_entries_are_listed_in_the_order_they_were_requested(pool: PgPool) {
    let studio = studio(pool).await;
    let mut later = window(&studio, &studio.ann, "2026-11-05T18:00:00", "2026-11-05T19:00:00");
    later["request_time"] = json!("2026-11-01T10:00:00");
    let mut earlier = window(&studio, &studio.ben, "2026-11-05T18:00:00", "2026-11-05T19:00:00");
    earlier["request_time"] = json!("2026-11-01T09:00:00");
    let (ann, ben) = (created(&studio.app, studio.org, "waitlistEntries", later).await, created(&studio.app, studio.org, "waitlistEntries", earlier).await);
    assert_eq!(ids(&studio.app, studio.org, "waitlistEntries", "").await, vec![ben["id"].clone(), ann["id"].clone()]);
    let own = filter(&format!("client_id = \"{}\"", studio.ann["id"].as_str().unwrap()));
    assert_eq!(ids(&studio.app, studio.org, "waitlistEntries", &own).await, vec![ann["id"].clone()]);
}

#[sqlx::test]
async fn leaving_deletes_the_entry_and_joining_twice_is_refused(pool: PgPool) {
    let studio = studio(pool).await;
    let body = window(&studio, &studio.ann, "2026-11-05T18:00:00", "2026-11-05T19:00:00");
    let entry = created(&studio.app, studio.org, "waitlistEntries", body.clone()).await;
    let (status, error) = call(&studio.app, "POST", &format!("/api/v1/orgs/{}/waitlistEntries", studio.org), Some(body.clone())).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::CONFLICT, json!("ALREADY_EXISTS")));
    let (status, left) = call(&studio.app, "DELETE", &path(studio.org, "waitlistEntries", &entry), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(left["delete_time"].is_string());
    assert!(ids(&studio.app, studio.org, "waitlistEntries", "").await.is_empty());
    created(&studio.app, studio.org, "waitlistEntries", body).await;
    assert_eq!(call(&studio.app, "PATCH", &path(studio.org, "waitlistEntries", &entry), Some(json!({}))).await.0, StatusCode::METHOD_NOT_ALLOWED);
}

#[sqlx::test]
async fn bad_input_is_refused(pool: PgPool) {
    let studio = studio(pool).await;
    let uri = format!("/api/v1/orgs/{}/waitlistEntries", studio.org);
    let cases = [
        (json!({}), StatusCode::BAD_REQUEST),
        (json!({"client_id": studio.ann["id"]}), StatusCode::BAD_REQUEST),
        (window(&studio, &studio.ann, "2026-11-05T19:00:00", "2026-11-05T18:00:00"), StatusCode::BAD_REQUEST),
        (json!({"client_id": studio.ann["id"], "target": {"somewhere": 1}}), StatusCode::BAD_REQUEST),
        (json!({"client_id": studio.ann["id"], "target": {"window": {"service_id": studio.class["id"], "start_time": "2026-11-05T18:00:00"}}}), StatusCode::BAD_REQUEST),
        (json!({"client_id": Uuid::new_v4(), "target": {"window": {"service_id": Uuid::new_v4(), "start_time": "2026-11-05T18:00:00", "end_time": "2026-11-05T19:00:00"}}}), StatusCode::NOT_FOUND),
        (json!({"client_id": studio.ann["id"], "target": {"window": {"service_id": Uuid::new_v4(), "start_time": "2026-11-05T18:00:00", "end_time": "2026-11-05T19:00:00"}}}), StatusCode::NOT_FOUND),
    ];
    for (body, expected) in cases {
        assert_eq!(call(&studio.app, "POST", &uri, Some(body.clone())).await.0, expected, "{body}");
    }
}

#[sqlx::test]
async fn an_org_sees_the_waits_of_the_orgs_grouped_under_it_and_not_those_of_others(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let wait = |org: Uuid, name: &'static str| {
        let app = app.clone();
        async move {
            let class = created(&app, org, "services", json!({"display_name": name, "price": {"currency_code": "USD", "units": "1", "nanos": 0}})).await;
            let client = created(&app, org, "clients", json!({"display_name": name})).await;
            let body = json!({"client_id": client["id"], "target": {"window": {"service_id": class["id"], "start_time": "2026-11-05T18:00:00", "end_time": "2026-11-05T19:00:00"}}});
            created(&app, org, "waitlistEntries", body).await
        }
    };
    let (below, elsewhere) = (wait(location, "below").await, wait(other, "elsewhere").await);
    assert_eq!(ids(&app, brand, "waitlistEntries", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "waitlistEntries", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}
