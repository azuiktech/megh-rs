//! Specification: services (steps, needs as AIP-160 filters, variants) over REST under `/api/v1/orgs/{org_id}/services`:
//! the request body is the entity, `PUT` replaces its data, `PATCH` merges the fields it names, answers are the entity and
//! a list is a page with its `data` and `next_page_token`; durations are ISO 8601 and errors are those of Google AIP-193.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn price(units: i64) -> Value {
    json!({"micros": units * 1_000_000, "currency": "USD"})
}

fn need(who: &str) -> Value {
    json!({"who": {"Filter": who}, "count": 1, "engagement": "engaged"})
}

fn step(duration: &str, needs: Vec<Value>) -> Value {
    json!({"duration": duration, "client": "present", "needs": needs, "attributes": {}})
}

fn service(org: Uuid, name: &str, steps: Vec<Value>) -> Value {
    json!({"org_id": org, "name": name, "mode": "appointment", "price": price(1), "steps": steps, "variants": [], "attributes": {}})
}

fn colour(org: Uuid) -> Value {
    let free = json!({"who": {"Filter": "role = \"stylist\""}, "count": 1, "engagement": "free"});
    let mut body = service(org, "Hair colour", vec![
        step("PT10M", vec![need("role = \"stylist\""), need("kind = \"chair\"")]),
        step("PT30M", vec![free, need("kind = \"chair\"")]),
        step("PT20M", vec![need("role = \"stylist\""), need("kind = \"chair\"")]),
    ]);
    body["price"] = price(120);
    body["variants"] = json!([{"when": {"Filter": "level = \"master\""}, "duration": {"type": "add", "value": "PT15M"}, "price_micros": {"type": "add", "value": 30_000_000}}]);
    body["attributes"] = json!({"category": "hair"});
    body
}

#[sqlx::test]
async fn a_service_is_created_from_the_entity_with_steps_needs_and_variants(pool: PgPool) {
    let org = clinic(&pool).await;
    let service = created(&booking_router(pool), org, "services", colour(org)).await;
    assert_eq!((service["name"].clone(), service["mode"].clone(), service["org_id"].clone()), (json!("Hair colour"), json!("appointment"), json!(org)));
    assert_eq!(service["price"], price(120));
    let steps = service["steps"].as_array().unwrap();
    assert_eq!(steps.iter().map(|s| s["duration"].clone()).collect::<Vec<_>>(), vec![json!("PT10M"), json!("PT30M"), json!("PT20M")]);
    assert_eq!((steps[1]["needs"][0]["engagement"].clone(), steps[0]["client"].clone()), (json!("free"), json!("present")));
    assert_eq!(service["variants"][0]["duration"], json!({"type": "add", "value": "PT15M"}));
    assert!(service["id"].is_string() && service["created_at"].is_string() && service["deleted_at"].is_null());
}

#[sqlx::test]
async fn a_service_is_read_back_listed_and_filtered(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let hair = created(&app, org, "services", colour(org)).await;
    let mut menu_body = service(org, "Hair", vec![]);
    menu_body["attributes"] = json!({"category": "menu"});
    let menu = created(&app, org, "services", menu_body).await;
    assert_eq!(call(&app, "GET", &path(org, "services", &hair), None).await, (StatusCode::OK, hair.clone()));
    assert_eq!(menu["steps"], json!([]));
    assert_eq!(ids(&app, org, "services", "").await, vec![hair["id"].clone(), menu["id"].clone()]);
    assert_eq!(ids(&app, org, "services", &filter("attributes.category = \"menu\"")).await, vec![menu["id"].clone()]);
    assert_eq!(ids(&app, org, "services", &filter("name = \"Hair c*\"")).await, vec![hair["id"].clone()]);
}

#[sqlx::test]
async fn a_put_replaces_the_data_and_a_patch_changes_only_the_fields_it_names(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let hair = created(&app, org, "services", colour(org)).await;
    let (status, replaced) = call(&app, "PUT", &path(org, "services", &hair), Some(service(org, "Cut", vec![step("PT30M", vec![need("kind = \"chair\"")])]))).await;
    assert_eq!((status, replaced["name"].clone(), replaced["steps"].as_array().unwrap().len(), replaced["variants"].clone()), (StatusCode::OK, json!("Cut"), 1, json!([])));
    assert_eq!((replaced["id"].clone(), replaced["created_at"].clone()), (hair["id"].clone(), hair["created_at"].clone()));
    let (_, repriced) = call(&app, "PATCH", &path(org, "services", &hair), Some(json!({"price": price(150)}))).await;
    assert_eq!((repriced["price"].clone(), repriced["name"].clone(), repriced["steps"].clone()), (price(150), json!("Cut"), replaced["steps"].clone()));
}

#[sqlx::test]
async fn a_delete_keeps_the_service_with_its_deleted_at(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let hair = created(&app, org, "services", colour(org)).await;
    let (status, deleted) = call(&app, "DELETE", &path(org, "services", &hair), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["deleted_at"].is_string());
    assert!(ids(&app, org, "services", "").await.is_empty());
    assert_eq!(ids(&app, org, "services", "?show_deleted=true").await.len(), 1);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    let with = |field: &str, value: Value| {
        let mut body = service(org, "S", vec![]);
        body[field] = value;
        body
    };
    let with_step = |duration: &str, needs: Vec<Value>| with("steps", json!([step(duration, needs)]));
    let cases = [
        json!({"org_id": org, "price": price(1)}),
        with("name", json!(" ")),
        with("price", json!({"micros": -5_000_000, "currency": "USD"})),
        with("mode", json!("teleport")),
        with("org_id", json!(other)),
        with_step("PT0S", vec![]),
        with_step("soon", vec![]),
        with_step("PT1M", vec![json!({"who": {"Filter": "role=\"stylist\""}, "count": 1, "engagement": "engaged"})]),
        with_step("PT1M", vec![json!({"who": {"Filter": "role = \"stylist\""}, "count": 0, "engagement": "engaged"})]),
        with_step("PT1M", vec![json!({"who": {"Filter": "role = \"stylist\""}, "count": 1, "engagement": "sometimes"})]),
        with("variants", json!([{"when": {"Filter": "level = "}, "duration": {"type": "add", "value": "PT1M"}, "price_micros": null}])),
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
    let (own, below, elsewhere) = (created(&app, brand, "services", service(brand, "a", vec![])).await, created(&app, location, "services", service(location, "b", vec![])).await, created(&app, other, "services", service(other, "c", vec![])).await);
    assert_eq!(ids(&app, brand, "services", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "services", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "services", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}
