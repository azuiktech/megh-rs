//! The authorizer on routes that have path parameters before the resource, as under an organization.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use tower::ServiceExt;
use uuid::Uuid;

use megh::{authorizer, Member};

const ORG: &str = "11111111-1111-1111-1111-111111111111";
const COURSE: &str = "22222222-2222-2222-2222-222222222222";

fn member(grants: &[&str]) -> Member {
    Member {
        id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: "member".to_string(),
        joined_at: Utc::now(),
        invited_by: None,
        grants: grants.iter().map(|g| g.to_string()).collect(),
    }
}

async fn inject_member(mut req: Request<Body>, next: Next) -> Response {
    let grants = req.headers().get("X-Grants").and_then(|v| v.to_str().ok()).map(str::to_owned);
    grants.into_iter().for_each(|g| {
        req.extensions_mut().insert(member(&g.split(',').collect::<Vec<_>>()));
    });
    next.run(req).await
}

fn router() -> Router {
    Router::new()
        .route("/organizations/{org_id}/courses", get(|| async { "list" }).post(|| async { "created" }))
        .route("/organizations/{org_id}/courses/{id}", get(|| async { "detail" }))
        .route("/organizations/{org_id}/courses/{course_id}/episodes", get(|| async { "episodes" }).post(|| async { "episode-created" }))
        .layer(middleware::from_fn(authorizer))
        .layer(middleware::from_fn(inject_member))
}

async fn status(method: &str, path: &str, grants: &str) -> StatusCode {
    let request = Request::builder().method(method).uri(path).header("X-Grants", grants).body(Body::empty()).unwrap();
    router().oneshot(request).await.unwrap().status()
}

#[tokio::test]
async fn a_resource_under_an_organization_is_checked_by_its_own_name() {
    let path = format!("/organizations/{ORG}/courses");
    assert_eq!(status("GET", &path, "courses:read").await, StatusCode::OK);
    assert_eq!(status("POST", &path, "courses:read").await, StatusCode::FORBIDDEN);
    assert_eq!(status("POST", &path, "courses:*").await, StatusCode::OK);
}

#[tokio::test]
async fn an_instance_under_an_organization_is_checked_with_its_id() {
    let path = format!("/organizations/{ORG}/courses/{COURSE}");
    assert_eq!(status("GET", &path, &format!("courses:read:{COURSE}")).await, StatusCode::OK);
    assert_eq!(status("GET", &path, "courses:read:another-course").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_nested_collection_is_checked_by_the_nested_name_not_the_parent() {
    let path = format!("/organizations/{ORG}/courses/{COURSE}/episodes");
    assert_eq!(status("GET", &path, "episodes:read").await, StatusCode::OK);
    assert_eq!(status("GET", &path, "courses:*").await, StatusCode::FORBIDDEN);
    assert_eq!(status("POST", &path, "episodes:read").await, StatusCode::FORBIDDEN);
}
