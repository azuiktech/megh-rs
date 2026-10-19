//! Specification: schedules and blockouts over REST under `/api/v1/orgs/{org_id}/schedules` and `/blockouts`: the same
//! entity of two kinds, the request body is the entity (a recurrence as RFC 5545 text, a span as ISO 8601), `PUT`
//! replaces its data, `PATCH` merges the fields it names, answers are the entity and a list is a page.

mod booking_support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use booking_support::rest::*;
use booking_support::*;
use chrono::{TimeZone, Utc};
use megh::booking::{booking_router, Availability, Axes, Calendar, Grouped, Interval};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const WEEKLY: &str = "DTSTART;TZID=America/New_York:20261104T150000\nRRULE:FREQ=WEEKLY;BYDAY=WE,TH";

fn schedule(org: Uuid, kind: &str, recurrence: &str, span: &str) -> Value {
    json!({"org_id": org, "service_id": null, "bookable_id": null, "kind": kind, "recurrence": recurrence, "span": span, "reason": null, "notes": null})
}

fn weekly(org: Uuid) -> Value {
    schedule(org, "open", WEEKLY, "PT3H")
}

fn blockout(org: Uuid) -> Value {
    let mut body = schedule(org, "blockout", "DTSTART;TZID=America/New_York:20261126T000000\nRRULE:FREQ=DAILY;COUNT=1", "PT23H59M");
    body["reason"] = json!("Thanksgiving");
    body["notes"] = json!("closed all day");
    body
}

#[sqlx::test]
async fn a_schedule_is_created_from_the_entity_in_the_org_of_the_path(pool: PgPool) {
    let org = clinic(&pool).await;
    let created = created(&booking_router(pool), org, "schedules", weekly(org)).await;
    // The rrule library writes the recurrence out in full.
    let written = "DTSTART;TZID=America/New_York:20261104T150000\nRRULE:FREQ=WEEKLY;BYHOUR=15;BYMINUTE=0;BYSECOND=0;BYDAY=WE,TH";
    assert_eq!((created["recurrence"].clone(), created["span"].clone(), created["org_id"].clone()), (json!(written), json!("PT3H"), json!(org)));
    assert!(created["id"].is_string() && created["created_at"].is_string() && created["deleted_at"].is_null());
}

#[sqlx::test]
async fn a_schedule_is_read_back_and_listed(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let created = created(&app, org, "schedules", weekly(org)).await;
    assert_eq!(call(&app, "GET", &path(org, "schedules", &created), None).await, (StatusCode::OK, created.clone()));
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules"), None).await;
    assert_eq!(listed, json!({"data": [created], "next_page_token": ""}));
}

#[sqlx::test]
async fn blockouts_are_apart_from_schedules_and_carry_a_reason(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let open = created(&app, org, "schedules", weekly(org)).await;
    let blocked = created(&app, org, "blockouts", blockout(org)).await;
    assert_eq!((blocked["reason"].clone(), blocked["notes"].clone(), blocked["kind"].clone()), (json!("Thanksgiving"), json!("closed all day"), json!("blockout")));
    assert_eq!((ids(&app, org, "schedules", "").await, ids(&app, org, "blockouts", "").await), (vec![open["id"].clone()], vec![blocked["id"].clone()]));
    assert_eq!(call(&app, "GET", &path(org, "schedules", &blocked), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", &path(org, "blockouts", &open), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/schedules"), Some(blockout(org))).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_in_the_aip_error_shape(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    let with = |field: &str, value: Value| {
        let mut body = weekly(org);
        body[field] = value;
        body
    };
    let cases = [
        with("recurrence", json!("not a recurrence")),
        with("span", json!("soon")),
        with("span", json!("PT0S")),
        with("kind", json!("sometimes")),
        with("org_id", json!(other)),
        json!({"org_id": org}),
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
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{}/schedules", Uuid::new_v4()), Some(weekly(org))).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_list_is_paged_with_an_opaque_token(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    for _ in 0..3 {
        created(&app, org, "schedules", weekly(org)).await;
    }
    let (_, first) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_size=2"), None).await;
    let token = first["next_page_token"].as_str().unwrap().to_string();
    assert_eq!((first["data"].as_array().unwrap().len(), token.is_empty()), (2, false));
    let (_, second) = call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_size=2&page_token={token}"), None).await;
    assert_eq!((second["data"].as_array().unwrap().len(), second["next_page_token"].clone()), (1, json!("")));
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_token=%25%25"), None).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?page_size=-1"), None).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_list_is_filtered_with_an_aip_160_expression(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool.clone());
    let (albert, beth) = (bookable(&pool, org, "albert", 1, &[]).await, bookable(&pool, org, "beth", 1, &[]).await);
    for bookable in [&albert, &beth] {
        let mut body = weekly(org);
        body["bookable_id"] = json!(bookable.id);
        created(&app, org, "schedules", body).await;
    }
    let found = ids(&app, org, "schedules", &filter(&format!("bookable_id = \"{}\"", albert.id.0))).await;
    assert_eq!(found.len(), 1);
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/schedules?filter=%28"), None).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_put_replaces_the_data_and_a_patch_changes_only_the_fields_it_names(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let created = created(&app, org, "blockouts", blockout(org)).await;
    let mut replacement = blockout(org);
    replacement["reason"] = json!("training");
    replacement["notes"] = json!("keep");
    let (status, replaced) = call(&app, "PUT", &path(org, "blockouts", &created), Some(replacement)).await;
    assert_eq!((status, replaced["reason"].clone(), replaced["id"].clone()), (StatusCode::OK, json!("training"), created["id"].clone()));
    let (_, patched) = call(&app, "PATCH", &path(org, "blockouts", &created), Some(json!({"notes": "changed"}))).await;
    assert_eq!((patched["reason"].clone(), patched["notes"].clone(), patched["recurrence"].clone()), (json!("training"), json!("changed"), created["recurrence"].clone()));
    let (_, longer) = call(&app, "PATCH", &path(org, "blockouts", &created), Some(json!({"span": "PT2H"}))).await;
    assert_eq!((longer["span"].clone(), longer["notes"].clone()), (json!("PT2H"), json!("changed")));
    assert_eq!(call(&app, "PATCH", &path(org, "blockouts", &created), Some(json!({"kind": "open"}))).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_delete_keeps_the_resource_with_its_deleted_at(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let created = created(&app, org, "schedules", weekly(org)).await;
    let uri = path(org, "schedules", &created);
    let (status, deleted) = call(&app, "DELETE", &uri, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["deleted_at"].is_string());
    assert!(ids(&app, org, "schedules", "").await.is_empty());
    assert_eq!(ids(&app, org, "schedules", "?show_deleted=true").await.len(), 1);
    assert!(call(&app, "GET", &uri, None).await.1["deleted_at"].is_string());
    assert_eq!(call(&app, "PATCH", &uri, Some(json!({"notes": "x"}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &uri, None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_schedules_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (created(&app, brand, "schedules", weekly(brand)).await, created(&app, location, "schedules", weekly(location)).await, created(&app, other, "schedules", weekly(other)).await);
    assert_eq!(ids(&app, brand, "schedules", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "schedules", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "schedules", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn another_orgs_bookable_cannot_be_used(pool: PgPool) {
    let (mine, theirs) = (clinic(&pool).await, clinic(&pool).await);
    let room = bookable(&pool, theirs, "room", 1, &[]).await;
    let app = booking_router(pool);
    let mut body = weekly(mine);
    body["bookable_id"] = json!(room.id);
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{mine}/schedules"), Some(body)).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{mine}/schedules"), Some(weekly(theirs))).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn schedules_and_blockouts_made_over_rest_drive_the_availability(pool: PgPool) {
    let org = clinic(&pool).await;
    let (room, cleaning) = (bookable(&pool, org, "room", 1, &[]).await, service(&pool, org, "cleaning", 30, None, is("kind", "room")).await);
    let app = booking_router(pool.clone());
    let mut open = weekly(org);
    open["bookable_id"] = json!(room.id);
    open["service_id"] = json!(cleaning.id);
    created(&app, org, "schedules", open).await;
    let mut meeting = schedule(org, "blockout", "DTSTART;TZID=America/New_York:20261112T160000\nRRULE:FREQ=DAILY;COUNT=1", "PT30M");
    meeting["bookable_id"] = json!(room.id);
    meeting["reason"] = json!("meeting");
    created(&app, org, "blockouts", meeting).await;
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
    let post = Request::post(format!("/api/v1/orgs/{org}/schedules")).header("content-type", "application/json").header("sec-fetch-site", "cross-site").body(Body::from(weekly(org).to_string())).unwrap();
    assert_eq!(app.clone().oneshot(post).await.unwrap().status(), StatusCode::FORBIDDEN);
    let get = Request::get(format!("/api/v1/orgs/{org}/schedules")).header("sec-fetch-site", "cross-site").body(Body::empty()).unwrap();
    assert_eq!(app.oneshot(get).await.unwrap().status(), StatusCode::OK);
}
