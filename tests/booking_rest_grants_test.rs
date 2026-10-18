//! Specification: the permissions of the booking routes come from their paths (`request_grants`): `org:read:{org_id}`
//! and then the resource and the action of the route, with the id when the path has one.

mod booking_support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::Router;
use booking_support::*;
use chrono::Utc;
use megh::booking::booking_router;
use megh::{authorizer, Member};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

fn member(grants: &str) -> Member {
    Member {
        id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: "member".into(),
        joined_at: Utc::now(),
        invited_by: None,
        grants: grants.split(' ').filter(|grant| !grant.is_empty()).map(String::from).collect(),
    }
}

async fn inject_member(mut request: Request<Body>, next: Next) -> Response {
    let grants = request.headers().get("x-grants").and_then(|value| value.to_str().ok()).map(str::to_owned);
    grants.into_iter().for_each(|grants| {
        request.extensions_mut().insert(member(&grants));
    });
    next.run(request).await
}

fn app(pool: PgPool) -> Router {
    booking_router(pool).layer(middleware::from_fn(authorizer)).layer(middleware::from_fn(inject_member))
}

async fn status(app: &Router, method: &str, uri: &str, grants: &str, body: &str) -> StatusCode {
    let request = Request::builder().method(method).uri(uri).header("x-grants", grants).header("content-type", "application/json").header("sec-fetch-site", "same-origin");
    app.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap().status()
}

const WEEKLY: &str = r#"{"days": ["MO"], "start_time": "09:00", "end_time": "12:00"}"#;

#[sqlx::test]
async fn nobody_is_let_in_without_a_member(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let request = Request::get(format!("/api/v1/org/{org}/schedules")).body(Body::empty()).unwrap();
    assert_eq!(app(pool).oneshot(request).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn creating_needs_the_org_and_the_create_grant(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let (app, uri) = (app(pool), format!("/api/v1/org/{org}/schedules"));
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{org} schedules:create"), WEEKLY).await, StatusCode::OK);
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{org}"), WEEKLY).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "POST", &uri, "schedules:create", WEEKLY).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{} schedules:create", Uuid::new_v4()), WEEKLY).await, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn a_blockout_is_its_own_resource_for_permissions(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let (app, uri) = (app(pool), format!("/api/v1/org/{org}/blockouts"));
    let body = r#"{"date": "2026-11-26", "start_time": "09:00", "end_time": "10:00", "reason": "meeting"}"#;
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{org} schedules:create"), body).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{org} blockouts:create"), body).await, StatusCode::OK);
}

#[sqlx::test]
async fn reading_deleting_and_listing_need_their_own_grants_on_the_instance(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let app = app(pool);
    let everything = format!("org:read:{org} schedules:*");
    let request = Request::post(format!("/api/v1/org/{org}/schedules")).header("x-grants", &everything).header("content-type", "application/json").header("sec-fetch-site", "same-origin").body(Body::from(WEEKLY)).unwrap();
    let bytes = axum::body::to_bytes(app.clone().oneshot(request).await.unwrap().into_body(), usize::MAX).await.unwrap();
    let id = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["id"].as_str().unwrap().to_string();
    let one = format!("/api/v1/org/{org}/schedules/{id}");
    assert_eq!(status(&app, "GET", &one, &format!("org:read:{org} schedules:read:{id}"), "").await, StatusCode::OK);
    assert_eq!(status(&app, "GET", &one, &format!("org:read:{org} schedules:read:{}", Uuid::new_v4()), "").await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "GET", &format!("/api/v1/org/{org}/schedules"), &format!("org:read:{org} schedules:read"), "").await, StatusCode::OK);
    assert_eq!(status(&app, "DELETE", &one, &format!("org:read:{org} schedules:read:{id}"), "").await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "DELETE", &one, &format!("org:read:{org} schedules:delete:{id}"), "").await, StatusCode::OK);
}

#[sqlx::test]
async fn searching_the_availability_needs_the_search_grant(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let (app, uri) = (app(pool), format!("/api/v1/org/{org}/availability:search"));
    let body = r#"{"parties": [], "start_time": "2026-11-04T09:00:00", "end_time": "2026-11-04T11:00:00"}"#;
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{org} availability:search"), body).await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&app, "POST", &uri, &format!("org:read:{org} availability:create"), body).await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "POST", &uri, "availability:search", body).await, StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn appointments_need_create_confirm_and_delete_grants_on_the_instance(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let (app, id) = (app(pool), Uuid::new_v4());
    let collection = format!("/api/v1/org/{org}/appointments");
    let read = format!("org:read:{org}");
    assert_eq!(status(&app, "POST", &collection, &format!("{read} appointments:create"), "{}").await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&app, "POST", &collection, &read, "{}").await, StatusCode::FORBIDDEN);
    let confirm = format!("{collection}/{id}:confirm");
    assert_eq!(status(&app, "POST", &confirm, &format!("{read} appointments:confirm:{id}"), "{}").await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&app, "POST", &confirm, &format!("{read} appointments:confirm:{}", Uuid::new_v4()), "{}").await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "POST", &confirm, &format!("{read} appointments:create"), "{}").await, StatusCode::FORBIDDEN);
    let one = format!("{collection}/{id}");
    assert_eq!(status(&app, "DELETE", &one, &format!("{read} appointments:delete:{id}"), "").await, StatusCode::NOT_FOUND);
    assert_eq!(status(&app, "DELETE", &one, &format!("{read} appointments:read:{id}"), "").await, StatusCode::FORBIDDEN);
    assert_eq!(status(&app, "GET", &collection, &format!("{read} appointments:read"), "").await, StatusCode::OK);
}
