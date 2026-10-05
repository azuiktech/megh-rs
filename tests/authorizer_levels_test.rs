//! The authorizer requires a grant for every resource level of the request.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use chrono::Utc;
use tower::ServiceExt;
use uuid::Uuid;

use megh::{authorizer, Member};

fn member(grants: &str) -> Member {
    Member {
        id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        role: "member".to_string(),
        joined_at: Utc::now(),
        invited_by: None,
        grants: grants.split(' ').filter(|g| !g.is_empty()).map(String::from).collect(),
    }
}

async fn inject_member(mut req: Request<Body>, next: Next) -> Response {
    let grants = req.headers().get("X-Grants").and_then(|v| v.to_str().ok()).map(str::to_owned);
    grants.into_iter().for_each(|g| {
        req.extensions_mut().insert(member(&g));
    });
    next.run(req).await
}

fn router() -> Router {
    let ok = || async { "ok" };
    let api = Router::new()
        .route("/organizations", get(ok).post(ok))
        .route("/organizations/{organization}", get(ok).post(ok).patch(ok))
        .route("/organizations/{organization}/courses", get(ok).post(ok))
        .route("/organizations/{organization}/courses/{course}", get(ok).patch(ok).post(ok))
        .route("/organizations/{organization}/courses/{course}/lessons/{lesson}", get(ok).patch(ok))
        .route("/organizations:batchCreate", post(ok))
        .layer(middleware::from_fn(authorizer));
    Router::new().nest("/v1", api).layer(middleware::from_fn(inject_member))
}

async fn status(method: &str, path: &str, grants: &str) -> StatusCode {
    let request = Request::builder().method(method).uri(path).header("X-Grants", grants).body(Body::empty()).unwrap();
    router().oneshot(request).await.unwrap().status()
}

const LESSON: &str = "/v1/organizations/o/courses/c/lessons/l";

#[tokio::test]
async fn every_level_must_be_granted() {
    assert_eq!(status("GET", LESSON, "organizations:read:o courses:read:c lessons:read:l").await, StatusCode::OK);
    assert_eq!(status("GET", LESSON, "courses:read:c lessons:read:l").await, StatusCode::FORBIDDEN);
    assert_eq!(status("GET", LESSON, "organizations:read:o lessons:read:l").await, StatusCode::FORBIDDEN);
    assert_eq!(status("GET", LESSON, "organizations:read:o courses:read:c").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_grant_on_one_level_implies_nothing_about_another() {
    assert_eq!(status("GET", LESSON, "organizations:* courses:read:c lessons:*").await, StatusCode::OK);
    assert_eq!(status("GET", LESSON, "organizations:*").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_grant_can_list_several_ids_and_ignores_case() {
    assert_eq!(status("GET", LESSON, "organizations:read:O,p,q courses:read:* lessons:read:l").await, StatusCode::OK);
    assert_eq!(status("GET", LESSON, "organizations:read:p,q courses:read:* lessons:read:l").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn writing_a_leaf_needs_read_on_the_parents_and_the_action_on_the_leaf() {
    assert_eq!(status("PATCH", LESSON, "organizations:read:o courses:read:c lessons:update:l").await, StatusCode::OK);
    assert_eq!(status("PATCH", LESSON, "organizations:read:o courses:read:c lessons:read:l").await, StatusCode::FORBIDDEN);
    assert_eq!(status("PATCH", LESSON, "organizations:update:o courses:update:c lessons:read:l").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn creating_in_a_collection_needs_a_grant_that_covers_every_instance() {
    let path = "/v1/organizations/o/courses";
    assert_eq!(status("POST", path, "organizations:read:o courses:create:*").await, StatusCode::OK);
    assert_eq!(status("POST", path, "organizations:read:o courses:create").await, StatusCode::OK);
    assert_eq!(status("POST", path, "organizations:read:o courses:create:c1").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_custom_verb_is_its_own_action() {
    let activate = "/v1/organizations/acme:activate";
    assert_eq!(status("POST", activate, "organizations:activate:acme").await, StatusCode::OK);
    assert_eq!(status("POST", activate, "organizations:create:acme").await, StatusCode::FORBIDDEN);
    assert_eq!(status("GET", "/v1/organizations/acme", "organizations:read,activate:*").await, StatusCode::OK);
    assert_eq!(status("POST", activate, "organizations:read,activate:*").await, StatusCode::OK);
}

#[tokio::test]
async fn a_custom_verb_on_a_collection_matches_its_action() {
    let path = "/v1/organizations:batchCreate";
    assert_eq!(status("POST", path, "organizations:read,write,batchCreate:*").await, StatusCode::OK);
    assert_eq!(status("POST", path, "organizations:read,write:*").await, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn requests_without_a_member_are_unauthorized() {
    let request = Request::builder().uri(LESSON).body(Body::empty()).unwrap();
    assert_eq!(router().oneshot(request).await.unwrap().status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn no_action_implies_another() {
    assert_eq!(status("PATCH", LESSON, "organizations:update:o courses:update:c lessons:update:l").await, StatusCode::FORBIDDEN, "update on a parent is not read");
    assert_eq!(status("GET", LESSON, "organizations:read:o courses:read:c lessons:update:l").await, StatusCode::FORBIDDEN, "update on the leaf is not read");
    assert_eq!(status("PATCH", LESSON, "organizations:read,update:o courses:read,update:c lessons:read,update:l").await, StatusCode::OK, "both listed, both allowed");
}
