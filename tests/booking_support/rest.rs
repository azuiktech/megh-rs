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
