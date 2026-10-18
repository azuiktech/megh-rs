//! Specification: docs/TPD-booking.md, R6 and R13: bookables (providers and rooms) over REST, in the manner of
//! Google AIP-131 to 135, 148, 158, 160, 164 and 193, under `/api/v1/orgs/{org_id}/bookables`.

mod booking_support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use booking_support::*;
use chrono::TimeZone;
use megh::booking::{booking_router, Availability, Axes, Calendar, Grouped, Interval};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri).header("content-type", "application/json").header("sec-fetch-site", "same-origin");
    let request = request.body(body.map_or_else(Body::empty, |body| Body::from(body.to_string()))).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap_or(Value::Null))
}

async fn clinic(pool: &PgPool) -> Uuid {
    let org = org(pool, "clinic").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(pool).await.unwrap();
    org
}

async fn created(app: &Router, org: Uuid, body: Value) -> Value {
    let (status, bookable) = call(app, "POST", &format!("/api/v1/orgs/{org}/bookables"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{bookable}");
    bookable
}

fn one(org: Uuid, bookable: &Value) -> String {
    format!("/api/v1/orgs/{org}/bookables/{}", bookable["id"].as_str().unwrap())
}

#[sqlx::test]
async fn a_bookable_is_created_with_its_tags_and_a_default_capacity_of_one(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let bookable = created(&app, org, json!({"display_name": "Dr Albert", "attributes": {"role": "dentist", "experience_years": 12}})).await;
    assert_eq!((bookable["display_name"].clone(), bookable["capacity"].clone(), bookable["org_id"].clone()), (json!("Dr Albert"), json!(1), json!(org)));
    assert_eq!(bookable["attributes"], json!({"role": "dentist", "experience_years": 12}));
    assert_eq!(bookable["name"], json!(format!("orgs/{org}/bookables/{}", bookable["id"].as_str().unwrap())));
    assert!(!bookable["create_time"].as_str().unwrap().ends_with('Z'));
    assert!(bookable["delete_time"].is_null());
}

#[sqlx::test]
async fn a_bookable_is_read_back_and_listed(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = created(&app, org, json!({"display_name": "Room 1", "capacity": 2})).await;
    let (status, read) = call(&app, "GET", &one(org, &room), None).await;
    assert_eq!((status, &read), (StatusCode::OK, &room));
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables"), None).await;
    assert_eq!(listed, json!({"bookables": [room], "next_page_token": ""}));
}

#[sqlx::test]
async fn a_list_is_filtered_by_the_tags_with_an_aip_160_expression(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    created(&app, org, json!({"display_name": "Albert", "attributes": {"role": "dentist", "experience_years": 12}})).await;
    created(&app, org, json!({"display_name": "Beth", "attributes": {"role": "hygienist", "experience_years": 3}})).await;
    created(&app, org, json!({"display_name": "Chair 1", "attributes": {"kind": "chair"}})).await;
    let names = |listed: &Value| listed["bookables"].as_array().unwrap().iter().map(|b| b["display_name"].clone()).collect::<Vec<_>>();
    let ask = |filter: &str| format!("/api/v1/orgs/{org}/bookables?filter={}", filter.replace(' ', "%20").replace('"', "%22").replace('=', "%3D").replace('>', "%3E").replace('.', "."));
    let (_, dentists) = call(&app, "GET", &ask("attributes.role = \"dentist\""), None).await;
    assert_eq!(names(&dentists), vec![json!("Albert")]);
    let (_, senior) = call(&app, "GET", &ask("attributes.experience_years >= 10"), None).await;
    assert_eq!(names(&senior), vec![json!("Albert")]);
    let (_, named) = call(&app, "GET", &ask("display_name = \"Ch*\""), None).await;
    assert_eq!(names(&named), vec![json!("Chair 1")]);
}

#[sqlx::test]
async fn a_list_is_paged(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    for index in 0..3 {
        created(&app, org, json!({"display_name": format!("Room {index}")})).await;
    }
    let (_, first) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?page_size=2"), None).await;
    let token = first["next_page_token"].as_str().unwrap().to_string();
    assert_eq!((first["bookables"].as_array().unwrap().len(), token.is_empty()), (2, false));
    let (_, second) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?page_size=2&page_token={token}"), None).await;
    assert_eq!((second["bookables"].as_array().unwrap().len(), second["next_page_token"].clone()), (1, json!("")));
}

#[sqlx::test]
async fn an_update_changes_the_masked_fields_only(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = created(&app, org, json!({"display_name": "Room 1", "capacity": 1, "attributes": {"view": "sea"}})).await;
    let (status, updated) = call(&app, "PATCH", &format!("{}?update_mask=capacity", one(org, &room)), Some(json!({"capacity": 3, "display_name": "ignored"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((updated["capacity"].clone(), updated["display_name"].clone()), (json!(3), json!("Room 1")));
    let (_, unmasked) = call(&app, "PATCH", &one(org, &room), Some(json!({"display_name": "Suite", "attributes": {"view": "garden"}}))).await;
    assert_eq!((unmasked["display_name"].clone(), unmasked["attributes"].clone(), unmasked["capacity"].clone()), (json!("Suite"), json!({"view": "garden"}), json!(3)));
    assert_eq!(unmasked["id"], room["id"]);
    assert_eq!(unmasked["create_time"], room["create_time"]);
}

#[sqlx::test]
async fn a_delete_keeps_the_resource_with_its_delete_time(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = created(&app, org, json!({"display_name": "Room 1"})).await;
    let (status, deleted) = call(&app, "DELETE", &one(org, &room), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["delete_time"].is_string());
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables"), None).await;
    assert!(listed["bookables"].as_array().unwrap().is_empty());
    let (_, shown) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?show_deleted=true"), None).await;
    assert_eq!(shown["bookables"].as_array().unwrap().len(), 1);
    assert!(call(&app, "GET", &one(org, &room), None).await.1["delete_time"].is_string());
    assert_eq!(call(&app, "PATCH", &one(org, &room), Some(json!({"capacity": 2}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &one(org, &room), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_in_the_aip_error_shape(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    let cases = [json!({}), json!({"display_name": "  "}), json!({"display_name": "Room", "capacity": 0}), json!({"display_name": "Room", "unknown": 1}), json!({"display_name": "Room", "org_id": other})];
    for body in cases {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/bookables"), Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!((error["error"]["code"].clone(), error["error"]["status"].clone()), (json!(400), json!("INVALID_ARGUMENT")), "{body}");
    }
    let (status, error) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables/{}", Uuid::new_v4()), None).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::NOT_FOUND, json!("NOT_FOUND")));
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{}/bookables", Uuid::new_v4()), Some(json!({"display_name": "Room"}))).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_bookables_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (created(&app, brand, json!({"display_name": "own"})).await, created(&app, location, json!({"display_name": "below"})).await, created(&app, other, json!({"display_name": "elsewhere"})).await);
    let ids = |listed: &Value| listed["bookables"].as_array().unwrap().iter().map(|b| b["id"].clone()).collect::<Vec<_>>();
    let (_, from_brand) = call(&app, "GET", &format!("/api/v1/orgs/{brand}/bookables"), None).await;
    let (_, from_location) = call(&app, "GET", &format!("/api/v1/orgs/{location}/bookables"), None).await;
    assert_eq!((ids(&from_brand), ids(&from_location)), (vec![own["id"].clone(), below["id"].clone()], vec![below["id"].clone()]));
    assert_eq!(call(&app, "GET", &one(brand, &elsewhere), None).await.0, StatusCode::NOT_FOUND);
    let below_in_brand = created(&app, brand, json!({"display_name": "placed below", "org_id": location})).await;
    assert_eq!(below_in_brand["org_id"], json!(location));
}

#[sqlx::test]
async fn bookables_made_over_rest_are_what_a_schedule_and_the_calendar_work_with(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool.clone());
    let room = created(&app, org, json!({"display_name": "Room 1", "attributes": {"kind": "room"}})).await;
    let cleaning = service(&pool, "cleaning", 30, None, is("kind", "room")).await;
    let schedule = json!({"bookable_id": room["id"], "service_id": cleaning.id, "date": "2026-11-12", "start_time": "09:00", "end_time": "10:00"});
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/schedules"), Some(schedule)).await.0, StatusCode::OK);
    let axes = Axes { org, service: cleaning.id, bookable: serde_json::from_value(room["id"].clone()).unwrap() };
    let day = Interval { start: chrono::Utc.with_ymd_and_hms(2026, 11, 12, 0, 0, 0).unwrap(), end: chrono::Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap() };
    let open = Calendar::new(pool).open(&axes, &day).await.unwrap();
    assert_eq!(open, vec![Interval { start: chrono::Utc.with_ymd_and_hms(2026, 11, 12, 14, 0, 0).unwrap(), end: chrono::Utc.with_ymd_and_hms(2026, 11, 12, 15, 0, 0).unwrap() }]);
}
