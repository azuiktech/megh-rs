//! Specification: docs/TPD-booking.md, R6: schedules and blockouts over REST, in the manner of Google AIP-131 to 135,
//! 148, 158, 160, 164, 193 and 142, under `/api/v1/orgs/{org_id}/...`.

mod booking_support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use booking_support::*;
use chrono::{TimeZone, Utc};
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
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn clinic(pool: &PgPool) -> Uuid {
    let org = org(pool, "clinic").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(pool).await.unwrap();
    org
}

fn weekly() -> Value {
    json!({"days": ["WE", "TH"], "start_time": "15:00", "end_time": "18:00", "start_date": "2026-11-04"})
}

async fn created(app: &Router, org: Uuid, collection: &str, body: Value) -> Value {
    let (status, resource) = call(app, "POST", &format!("/api/v1/orgs/{org}/{collection}"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{resource}");
    resource
}

#[sqlx::test]
async fn a_schedule_is_created_from_local_hours_in_the_orgs_time_zone(pool: PgPool) {
    let org = clinic(&pool).await;
    let schedule = created(&booking_router(pool), org, "schedules", weekly()).await;
    assert_eq!(schedule["recurrence"], "DTSTART;TZID=America/New_York:20261104T150000\nRRULE:FREQ=WEEKLY;BYDAY=WE,TH");
    assert_eq!(schedule["span"], "10800s");
    assert_eq!(schedule["org_id"], json!(org));
    assert_eq!(schedule["name"], format!("orgs/{org}/schedules/{}", schedule["id"].as_str().unwrap()));
    assert!(!schedule["create_time"].as_str().unwrap().ends_with('Z'));
    assert!(schedule["delete_time"].is_null());
}

#[sqlx::test]
async fn a_recurrence_with_a_span_is_accepted_as_it_is(pool: PgPool) {
    let org = clinic(&pool).await;
    let body = json!({"recurrence": "DTSTART;TZID=America/New_York:20261104T090000\nRRULE:FREQ=DAILY", "span": "28800s"});
    let schedule = created(&booking_router(pool), org, "schedules", body).await;
    assert_eq!(schedule["span"], "28800s");
}

#[sqlx::test]
async fn a_schedule_is_read_back_and_listed(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let schedule = created(&app, org, "schedules", weekly()).await;
    let (status, read) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules/{}", schedule["id"].as_str().unwrap()), None).await;
    assert_eq!((status, &read), (StatusCode::OK, &schedule));
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules"), None).await;
    assert_eq!(listed, json!({"schedules": [schedule], "next_page_token": ""}));
}

#[sqlx::test]
async fn blockouts_are_apart_from_schedules_and_carry_a_reason(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let schedule = created(&app, org, "schedules", weekly()).await;
    let body = json!({"date": "2026-11-26", "start_time": "00:00", "end_time": "23:59", "reason": "Thanksgiving", "notes": "closed all day"});
    let blockout = created(&app, org, "blockouts", body).await;
    assert_eq!((blockout["reason"].clone(), blockout["notes"].clone()), (json!("Thanksgiving"), json!("closed all day")));
    let (_, schedules) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules"), None).await;
    let (_, blockouts) = call(&app, "GET", &format!("/api/v1/orgs/{org}/blockouts"), None).await;
    assert_eq!((schedules["schedules"].as_array().unwrap().len(), blockouts["blockouts"].as_array().unwrap().len()), (1, 1));
    let (status, _) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules/{}", blockout["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = call(&app, "GET", &format!("/api/v1/orgs/{org}/blockouts/{}", schedule["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_in_the_aip_error_shape(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let cases = [
        json!({"recurrence": "not a recurrence", "span": "3600s"}),
        json!({"days": ["WE"], "start_time": "18:00", "end_time": "15:00"}),
        json!({"days": ["XX"], "start_time": "15:00", "end_time": "18:00"}),
        json!({"days": ["WE"], "date": "2026-11-04", "start_time": "15:00", "end_time": "18:00"}),
        json!({"start_time": "15:00", "end_time": "18:00"}),
        json!({"recurrence": "DTSTART:20261104T150000Z\nRRULE:FREQ=DAILY", "span": "soon"}),
        json!({"unknown_field": 1}),
    ];
    for body in cases {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/schedules"), Some(body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert_eq!((error["error"]["code"].clone(), error["error"]["status"].clone()), (json!(400), json!("INVALID_ARGUMENT")), "{body}");
        assert!(error["error"]["message"].is_string());
    }
}

#[sqlx::test]
async fn a_missing_resource_or_org_is_not_found(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let (status, error) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules/{}", Uuid::new_v4()), None).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::NOT_FOUND, json!("NOT_FOUND")));
    let (status, _) = call(&app, "POST", &format!("/api/v1/orgs/{}/schedules", Uuid::new_v4()), Some(weekly())).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_list_is_paged_with_an_opaque_token(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    for _ in 0..3 {
        created(&app, org, "schedules", weekly()).await;
    }
    let (_, first) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_size=2"), None).await;
    let token = first["next_page_token"].as_str().unwrap().to_string();
    assert_eq!((first["schedules"].as_array().unwrap().len(), token.is_empty()), (2, false));
    let (_, second) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_size=2&page_token={token}"), None).await;
    assert_eq!((second["schedules"].as_array().unwrap().len(), second["next_page_token"].clone()), (1, json!("")));
    let (status, _) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_token=%25%25"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_size=-1"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_list_is_filtered_with_an_aip_160_expression(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool.clone());
    let (albert, beth) = (bookable(&pool, org, "albert", 1, &[]).await, bookable(&pool, org, "beth", 1, &[]).await);
    for bookable in [&albert, &beth] {
        created(&app, org, "schedules", json!({"bookable_id": bookable.id, "days": ["MO"], "start_time": "09:00", "end_time": "12:00"})).await;
    }
    let filter = format!("bookable_id = \"{}\"", albert.id.0);
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?filter={}", filter.replace(' ', "%20").replace('"', "%22")), None).await;
    let found = listed["schedules"].as_array().unwrap();
    assert_eq!((found.len(), found[0]["bookable_id"].clone()), (1, json!(albert.id)));
    let (status, _) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?filter=%28"), None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn an_update_changes_the_masked_fields_only(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let blockout = created(&app, org, "blockouts", json!({"date": "2026-11-26", "start_time": "09:00", "end_time": "10:00", "reason": "meeting", "notes": "keep"})).await;
    let uri = format!("/api/v1/orgs/{org}/blockouts/{}", blockout["id"].as_str().unwrap());
    let (status, updated) = call(&app, "PATCH", &format!("{uri}?update_mask=reason"), Some(json!({"reason": "training", "notes": "ignored"}))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((updated["reason"].clone(), updated["notes"].clone()), (json!("training"), json!("keep")));
    let (_, unmasked) = call(&app, "PATCH", &uri, Some(json!({"notes": "changed"}))).await;
    assert_eq!((unmasked["reason"].clone(), unmasked["notes"].clone()), (json!("training"), json!("changed")));
    assert_eq!(unmasked["recurrence"], blockout["recurrence"]);
}

#[sqlx::test]
async fn an_update_can_change_the_hours(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let schedule = created(&app, org, "schedules", weekly()).await;
    let uri = format!("/api/v1/orgs/{org}/schedules/{}", schedule["id"].as_str().unwrap());
    let (_, updated) = call(&app, "PATCH", &uri, Some(json!({"days": ["MO"], "start_time": "09:00", "end_time": "10:30", "start_date": "2026-11-02"}))).await;
    assert_eq!((updated["span"].clone(), updated["recurrence"].clone()), (json!("5400s"), json!("DTSTART;TZID=America/New_York:20261102T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO")));
    let (_, longer) = call(&app, "PATCH", &uri, Some(json!({"span": "7200s"}))).await;
    assert_eq!((longer["span"].clone(), longer["recurrence"].clone()), (json!("7200s"), updated["recurrence"].clone()));
}

#[sqlx::test]
async fn a_delete_keeps_the_resource_with_its_delete_time(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let schedule = created(&app, org, "schedules", weekly()).await;
    let uri = format!("/api/v1/orgs/{org}/schedules/{}", schedule["id"].as_str().unwrap());
    let (status, deleted) = call(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["delete_time"].is_string());
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules"), None).await;
    assert!(listed["schedules"].as_array().unwrap().is_empty());
    let (_, shown) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?show_deleted=true"), None).await;
    assert_eq!(shown["schedules"].as_array().unwrap().len(), 1);
    let (_, read) = call(&app, "GET", &uri, None).await;
    assert!(read["delete_time"].is_string());
    assert_eq!(call(&app, "PATCH", &uri, Some(json!({"notes": "x"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &uri, None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_schedules_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (created(&app, brand, "schedules", weekly()).await, created(&app, location, "schedules", weekly()).await, created(&app, other, "schedules", weekly()).await);
    let ids = |listed: &Value| listed["schedules"].as_array().unwrap().iter().map(|schedule| schedule["id"].clone()).collect::<Vec<_>>();
    let (_, brand_list) = call(&app, "GET", &format!("/api/v1/orgs/{brand}/schedules"), None).await;
    let (_, location_list) = call(&app, "GET", &format!("/api/v1/orgs/{location}/schedules"), None).await;
    assert_eq!((ids(&brand_list), ids(&location_list)), (vec![own["id"].clone(), below["id"].clone()], vec![below["id"].clone()]));
    let (status, _) = call(&app, "GET", &format!("/api/v1/orgs/{brand}/schedules/{}", elsewhere["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn another_orgs_bookable_cannot_be_used(pool: PgPool) {
    let (mine, theirs) = (clinic(&pool).await, clinic(&pool).await);
    let room = bookable(&pool, theirs, "room", 1, &[]).await;
    let app = booking_router(pool);
    let (status, _) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/schedules"), Some(json!({"bookable_id": room.id, "days": ["MO"], "start_time": "09:00", "end_time": "12:00"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, _) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/schedules"), Some(json!({"org_id": theirs, "days": ["MO"], "start_time": "09:00", "end_time": "12:00"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn schedules_and_blockouts_made_over_rest_drive_the_availability(pool: PgPool) {
    let org = clinic(&pool).await;
    let (room, cleaning) = (bookable(&pool, org, "room", 1, &[]).await, service(&pool, org, "cleaning", 30, None, is("kind", "room")).await);
    let app = booking_router(pool.clone());
    created(&app, org, "schedules", json!({"bookable_id": room.id, "service_id": cleaning.id, "days": ["WE", "TH"], "start_time": "15:00", "end_time": "18:00", "start_date": "2026-11-04"})).await;
    created(&app, org, "blockouts", json!({"bookable_id": room.id, "date": "2026-11-12", "start_time": "16:00", "end_time": "16:30", "reason": "meeting"})).await;
    let axes = Axes { org, service: cleaning.id, bookable: room.id };
    let day = Interval { start: Utc.with_ymd_and_hms(2026, 11, 12, 0, 0, 0).unwrap(), end: Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap() };
    let at = |hour, minute| Utc.with_ymd_and_hms(2026, 11, 12, hour, minute, 0).unwrap();
    let open = Calendar::new(pool).open(&axes, &day).await.unwrap();
    assert_eq!(open, vec![Interval { start: at(20, 0), end: at(21, 0) }, Interval { start: at(21, 30), end: at(23, 0) }]);
}

#[sqlx::test]
async fn a_write_from_another_site_is_refused_and_a_read_is_not(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let post = Request::post(format!("/api/v1/orgs/{org}/schedules")).header("content-type", "application/json").header("sec-fetch-site", "cross-site").body(Body::from(weekly().to_string())).unwrap();
    assert_eq!(app.clone().oneshot(post).await.unwrap().status(), StatusCode::FORBIDDEN);
    let get = Request::get(format!("/api/v1/orgs/{org}/schedules")).header("sec-fetch-site", "cross-site").body(Body::empty()).unwrap();
    assert_eq!(app.oneshot(get).await.unwrap().status(), StatusCode::OK);
}
