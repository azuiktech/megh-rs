//! Specification: AIP-132: `order_by` on a list is a comma separated list of fields, each with an optional `desc`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use megh::booking::booking_router;
use serde_json::{json, Value};
use sqlx::PgPool;

async fn rooms(app: &axum::Router, org: uuid::Uuid) {
    for (name, capacity) in [("Beta", 2), ("Alpha", 2), ("Gamma", 1)] {
        rest_bookable(app, org, name, capacity, json!({})).await;
    }
}

async fn names(app: &axum::Router, org: uuid::Uuid, order: &str) -> (StatusCode, Vec<Value>) {
    let (status, listed) = call(app, "GET", &format!("/api/v1/orgs/{org}/bookables?order_by={order}"), None).await;
    (status, listed["data"].as_array().map(|items| items.iter().map(|item| item["name"].clone()).collect()).unwrap_or_default())
}

#[sqlx::test]
async fn a_list_is_ordered_by_one_field_or_several_and_either_way(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    rooms(&app, org).await;
    assert_eq!(names(&app, org, "").await.1, vec![json!("Beta"), json!("Alpha"), json!("Gamma")]);
    assert_eq!(names(&app, org, "name").await.1, vec![json!("Alpha"), json!("Beta"), json!("Gamma")]);
    assert_eq!(names(&app, org, "name%20desc").await.1, vec![json!("Gamma"), json!("Beta"), json!("Alpha")]);
    assert_eq!(names(&app, org, "capacity%20desc,name").await.1, vec![json!("Alpha"), json!("Beta"), json!("Gamma")]);
    assert_eq!(names(&app, org, "capacity,name%20desc").await.1, vec![json!("Gamma"), json!("Beta"), json!("Alpha")]);
    assert_eq!(names(&app, org, "created_at%20desc").await.1, vec![json!("Gamma"), json!("Alpha"), json!("Beta")]);
}

#[sqlx::test]
async fn an_order_applies_before_the_page_is_cut(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    rooms(&app, org).await;
    let (_, first) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?order_by=name&page_size=2"), None).await;
    assert_eq!(first["data"].as_array().unwrap().iter().map(|b| b["name"].clone()).collect::<Vec<_>>(), vec![json!("Alpha"), json!("Beta")]);
}

#[sqlx::test]
async fn a_field_that_does_not_exist_or_a_bad_direction_is_refused(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    rooms(&app, org).await;
    for order in ["colour", "name%20sideways", "name%20desc%20now", "name,,capacity"] {
        let (status, error) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?order_by={order}"), None).await;
        assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")), "{order}");
    }
}

#[sqlx::test]
async fn times_are_ordered_as_times_in_every_list(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    for date in ["2026-11-12", "2026-11-04"] {
        created(&app, org, "schedules", json!({"date": date, "start_time": "09:00", "end_time": "10:00", "org_id": org})).await;
    }
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?order_by=recurrence"), None).await;
    let recurrences: Vec<_> = listed["schedules"].as_array().unwrap().iter().map(|s| s["recurrence"].as_str().unwrap().to_string()).collect();
    assert!(recurrences[0].contains("20261104") && recurrences[1].contains("20261112"));
    let (_, latest) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?order_by=create_time%20desc"), None).await;
    assert!(latest["schedules"][0]["recurrence"].as_str().unwrap().contains("20261104"));
}
