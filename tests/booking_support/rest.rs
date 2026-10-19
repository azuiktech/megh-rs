//! Calling the booking router in tests.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

pub async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
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

/// An org whose time zone is New York.
pub async fn clinic(pool: &PgPool) -> Uuid {
    let org = super::org(pool, "clinic").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(pool).await.unwrap();
    org
}

/// Creates a resource and returns it.
pub async fn created(app: &Router, org: Uuid, collection: &str, body: Value) -> Value {
    let (status, resource) = call(app, "POST", &format!("/api/v1/orgs/{org}/{collection}"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{resource}");
    resource
}

/// Creates a bookable of the org.
pub async fn rest_bookable(app: &Router, org: Uuid, name: impl Into<Value>, capacity: u32, attributes: Value) -> Value {
    created(app, org, "bookables", serde_json::json!({"org_id": org, "name": name.into(), "capacity": capacity, "attributes": attributes})).await
}

/// Creates a client of the org.
pub async fn rest_client(app: &Router, org: Uuid, name: &str, attributes: Value) -> Value {
    created(app, org, "clients", serde_json::json!({"org_id": org, "name": name, "first_name": name, "last_name": "Client", "attributes": attributes})).await
}

/// Creates a service of the org with no steps.
pub async fn rest_service(app: &Router, org: Uuid, name: &str, units: i64) -> Value {
    let price = serde_json::json!({"micros": units * 1_000_000, "currency": "USD"});
    created(app, org, "services", serde_json::json!({"org_id": org, "name": name, "mode": "appointment", "price": price, "steps": [], "variants": [], "attributes": {}})).await
}

/// A schedule opening the bookable from 9:00 to 17:00 New York time on 2026-11-04 (14:00 to 22:00 UTC).
pub async fn rest_open(app: &Router, org: Uuid, bookable: &Value) -> Value {
    let recurrence = "DTSTART;TZID=America/New_York:20261104T090000\nRRULE:FREQ=DAILY;COUNT=1";
    created(app, org, "schedules", serde_json::json!({"org_id": org, "service_id": null, "bookable_id": bookable["id"], "kind": "open", "recurrence": recurrence, "span": "PT8H", "reason": null, "notes": null})).await
}

/// The request of a search for one party of one service between two UTC times of 2026-11-04.
pub fn search_for(client: Option<&Value>, service: &Value, from: &str, to: &str) -> Value {
    let party = serde_json::json!({"client": client.map(|client| client["id"].clone()), "services": [{"service": service["id"]}]});
    serde_json::json!({"parties": [party], "within": {"start": format!("2026-11-04T{from}:00Z"), "end": format!("2026-11-04T{to}:00Z")}})
}

/// A payment of `amount` (such as `{"type": "uses", "value": 1}`) by the client with the method.
pub fn rest_payment(client: &Value, method: &Value, amount: Value) -> Value {
    serde_json::json!({"payer_id": client["id"], "method": method["id"], "amount": amount})
}

/// Searches with the request and books the first slot found, with a payment for each placement.
pub async fn rest_book(app: &Router, org: Uuid, request: Value, payments: Vec<Value>, request_id: &str) -> (StatusCode, Value) {
    let (_, found) = call(app, "POST", &format!("/api/v1/orgs/{org}/availability:search"), Some(request.clone())).await;
    let body = serde_json::json!({"request": request, "slot": found["slots"][0], "payments": payments, "request_id": request_id});
    call(app, "POST", &format!("/api/v1/orgs/{org}/appointments"), Some(body)).await
}

pub fn path(org: Uuid, collection: &str, resource: &Value) -> String {
    format!("/api/v1/orgs/{org}/{collection}/{}", resource["id"].as_str().unwrap())
}

/// The ids in the list a call to the collection answers.
pub async fn ids(app: &Router, org: Uuid, collection: &str, query: &str) -> Vec<Value> {
    let (_, listed) = call(app, "GET", &format!("/api/v1/orgs/{org}/{collection}{query}"), None).await;
    listed.get("data").unwrap_or(&listed[collection]).as_array().unwrap().iter().map(|item| item["id"].clone()).collect()
}

/// A filter in a query string.
pub fn filter(expression: &str) -> String {
    let encoded: String = expression.chars().map(|c| match c {
        ' ' => "%20".to_string(),
        '"' => "%22".to_string(),
        '=' => "%3D".to_string(),
        '>' => "%3E".to_string(),
        '<' => "%3C".to_string(),
        ':' => "%3A".to_string(),
        other => other.to_string(),
    }).collect();
    format!("?filter={encoded}")
}
