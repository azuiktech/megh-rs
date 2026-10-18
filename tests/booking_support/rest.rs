//! Calling the booking router in tests.

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

pub async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri).header("content-type", "application/json").header("sec-fetch-site", "same-origin");
    let request = request.body(body.map_or_else(Body::empty, |body| Body::from(body.to_string()))).unwrap();
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

pub fn path(org: Uuid, collection: &str, resource: &Value) -> String {
    format!("/api/v1/orgs/{org}/{collection}/{}", resource["id"].as_str().unwrap())
}

/// The ids in the list a call to the collection answers.
pub async fn ids(app: &Router, org: Uuid, collection: &str, query: &str) -> Vec<Value> {
    let (_, listed) = call(app, "GET", &format!("/api/v1/orgs/{org}/{collection}{query}"), None).await;
    listed[collection].as_array().unwrap().iter().map(|item| item["id"].clone()).collect()
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
