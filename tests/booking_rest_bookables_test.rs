//! Specification: bookables (providers and rooms) over REST under `/api/v1/orgs/{org_id}/bookables`: the request
//! body is the entity, an update is a `PUT` of the whole entity, answers are the entity and a list is a page with
//! its `data` and `next_page_token`; errors are those of Google AIP-193.

mod booking_support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use booking_support::rest::rest_bookable;
use booking_support::*;
use chrono::TimeZone;
use megh::booking::{booking_router, Availability, Axes, Calendar, Grouped, Interval};
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

async fn clinic(pool: &PgPool) -> Uuid {
    let org = org(pool, "clinic").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(pool).await.unwrap();
    org
}

fn body(org: Uuid, name: &str, capacity: u32, attributes: Value) -> Value {
    json!({"org_id": org, "name": name, "capacity": capacity, "attributes": attributes})
}

fn one(org: Uuid, bookable: &Value) -> String {
    format!("/api/v1/orgs/{org}/bookables/{}", bookable["id"].as_str().unwrap())
}

fn names(listed: &Value) -> Vec<Value> {
    listed["data"].as_array().unwrap().iter().map(|b| b["name"].clone()).collect()
}

#[sqlx::test]
async fn a_bookable_is_created_from_the_entity_without_its_id(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let bookable = rest_bookable(&app, org, "Dr Albert", 1, json!({"role": "dentist", "experience_years": 12})).await;
    assert_eq!((bookable["name"].clone(), bookable["capacity"].clone(), bookable["org_id"].clone()), (json!("Dr Albert"), json!(1), json!(org)));
    assert_eq!(bookable["attributes"], json!({"role": "dentist", "experience_years": 12}));
    assert!(bookable["id"].is_string() && bookable["created_at"].is_string() && bookable["deleted_at"].is_null());
}

#[sqlx::test]
async fn a_bookable_is_read_back_and_listed(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = rest_bookable(&app, org, "Room 1", 2, json!({})).await;
    let (status, read) = call(&app, "GET", &one(org, &room), None).await;
    assert_eq!((status, &read), (StatusCode::OK, &room));
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables"), None).await;
    assert_eq!(listed, json!({"data": [room], "next_page_token": ""}));
}

#[sqlx::test]
async fn a_list_is_filtered_by_the_tags_with_an_aip_160_expression(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    rest_bookable(&app, org, "Albert", 1, json!({"role": "dentist", "experience_years": 12})).await;
    rest_bookable(&app, org, "Beth", 1, json!({"role": "hygienist", "experience_years": 3})).await;
    rest_bookable(&app, org, "Chair 1", 1, json!({"kind": "chair"})).await;
    let ask = |filter: &str| format!("/api/v1/orgs/{org}/bookables?filter={}", filter.replace(' ', "%20").replace('"', "%22").replace('=', "%3D").replace('>', "%3E"));
    let (_, dentists) = call(&app, "GET", &ask("attributes.role = \"dentist\""), None).await;
    assert_eq!(names(&dentists), vec![json!("Albert")]);
    let (_, senior) = call(&app, "GET", &ask("attributes.experience_years >= 10"), None).await;
    assert_eq!(names(&senior), vec![json!("Albert")]);
    let (_, named) = call(&app, "GET", &ask("name = \"Ch*\""), None).await;
    assert_eq!(names(&named), vec![json!("Chair 1")]);
}

#[sqlx::test]
async fn a_list_is_paged(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    for index in 0..3 {
        rest_bookable(&app, org, format!("Room {index}"), 1, json!({})).await;
    }
    let (_, first) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?page_size=2"), None).await;
    let token = first["next_page_token"].as_str().unwrap().to_string();
    assert_eq!((first["data"].as_array().unwrap().len(), token.is_empty()), (2, false));
    let (_, second) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?page_size=2&page_token={token}"), None).await;
    assert_eq!((second["data"].as_array().unwrap().len(), second["next_page_token"].clone()), (1, json!("")));
}

#[sqlx::test]
async fn a_put_replaces_the_data_and_keeps_the_id_and_creation(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = rest_bookable(&app, org, "Room 1", 1, json!({"view": "sea"})).await;
    let (status, replaced) = call(&app, "PUT", &one(org, &room), Some(body(org, "Suite", 3, json!({"view": "garden"})))).await;
    assert_eq!(status, StatusCode::OK, "{replaced}");
    assert_eq!((replaced["name"].clone(), replaced["capacity"].clone(), replaced["attributes"].clone()), (json!("Suite"), json!(3), json!({"view": "garden"})));
    assert_eq!((replaced["id"].clone(), replaced["created_at"].clone()), (room["id"].clone(), room["created_at"].clone()));
    assert_eq!(call(&app, "GET", &one(org, &room), None).await.1, replaced);
}

#[sqlx::test]
async fn a_patch_changes_only_the_fields_it_names(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = rest_bookable(&app, org, "Room 1", 1, json!({"view": "sea"})).await;
    let (status, patched) = call(&app, "PATCH", &one(org, &room), Some(json!({"capacity": 4}))).await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!((patched["name"].clone(), patched["capacity"].clone(), patched["attributes"].clone()), (json!("Room 1"), json!(4), json!({"view": "sea"})));
    assert_eq!(patched["id"], room["id"]);
}

#[sqlx::test]
async fn a_put_or_patch_that_breaks_the_rules_is_refused_and_changes_nothing(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = rest_bookable(&app, org, "Room 1", 2, json!({})).await;
    assert_eq!(call(&app, "PUT", &one(org, &room), Some(body(org, "  ", 2, json!({})))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "PATCH", &one(org, &room), Some(json!({"capacity": 0}))).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "GET", &one(org, &room), None).await.1, room);
}

#[sqlx::test]
async fn a_delete_keeps_the_bookable_with_its_deleted_at(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = rest_bookable(&app, org, "Room 1", 1, json!({})).await;
    let (status, deleted) = call(&app, "DELETE", &one(org, &room), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["deleted_at"].is_string());
    let (_, listed) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables"), None).await;
    assert!(listed["data"].as_array().unwrap().is_empty());
    let (_, shown) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables?show_deleted=true"), None).await;
    assert_eq!(shown["data"].as_array().unwrap().len(), 1);
    assert!(call(&app, "GET", &one(org, &room), None).await.1["deleted_at"].is_string());
    assert_eq!(call(&app, "PUT", &one(org, &room), Some(body(org, "Room 1", 1, json!({})))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "PATCH", &one(org, &room), Some(json!({"capacity": 2}))).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &one(org, &room), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_in_the_aip_error_shape(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let cases = [json!({}), body(org, "  ", 1, json!({})), body(org, "Room", 0, json!({}))];
    for case in cases {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/bookables"), Some(case.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{case}");
        assert_eq!((error["error"]["code"].clone(), error["error"]["status"].clone()), (json!(400), json!("INVALID_ARGUMENT")), "{case}");
    }
    let (status, error) = call(&app, "GET", &format!("/api/v1/orgs/{org}/bookables/{}", Uuid::new_v4()), None).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::NOT_FOUND, json!("NOT_FOUND")));
    let missing = Uuid::new_v4();
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{missing}/bookables"), Some(body(missing, "Room", 1, json!({})))).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_bookables_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (rest_bookable(&app, brand, "own", 1, json!({})).await, rest_bookable(&app, location, "below", 1, json!({})).await, rest_bookable(&app, other, "elsewhere", 1, json!({})).await);
    let ids = |listed: &Value| listed["data"].as_array().unwrap().iter().map(|b| b["id"].clone()).collect::<Vec<_>>();
    let (_, from_brand) = call(&app, "GET", &format!("/api/v1/orgs/{brand}/bookables"), None).await;
    let (_, from_location) = call(&app, "GET", &format!("/api/v1/orgs/{location}/bookables"), None).await;
    assert_eq!((ids(&from_brand), ids(&from_location)), (vec![own["id"].clone(), below["id"].clone()], vec![below["id"].clone()]));
    assert_eq!(call(&app, "GET", &one(brand, &elsewhere), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!((below["org_id"].clone(), own["org_id"].clone()), (json!(location), json!(brand)));
}

#[sqlx::test]
async fn bookables_made_over_rest_are_what_a_schedule_and_the_calendar_work_with(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool.clone());
    let room = rest_bookable(&app, org, "Room 1", 1, json!({"kind": "room"})).await;
    let cleaning = service(&pool, org, "cleaning", 30, None, is("kind", "room")).await;
    let recurrence = "DTSTART;TZID=America/New_York:20261112T090000\nRRULE:FREQ=DAILY;COUNT=1";
    let schedule = json!({"org_id": org, "service_id": cleaning.id, "bookable_id": room["id"], "kind": "open", "recurrence": recurrence, "span": "PT1H", "reason": null, "notes": null});
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/schedules"), Some(schedule)).await.0, StatusCode::OK);
    let axes = Axes { org, service: cleaning.id, bookable: serde_json::from_value(room["id"].clone()).unwrap() };
    let day = Interval { start: chrono::Utc.with_ymd_and_hms(2026, 11, 12, 0, 0, 0).unwrap(), end: chrono::Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap() };
    let open = Calendar::new(pool).open(&axes, &day).await.unwrap();
    assert_eq!(open, vec![Interval { start: chrono::Utc.with_ymd_and_hms(2026, 11, 12, 14, 0, 0).unwrap(), end: chrono::Utc.with_ymd_and_hms(2026, 11, 12, 15, 0, 0).unwrap() }]);
}
