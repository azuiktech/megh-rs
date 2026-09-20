//! Integration tests verifying course authorization using wildcard educator grants.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use chrono::Utc;
use tower::ServiceExt;
use uuid::Uuid;

use megh::{authorizer, OrgMember};

fn create_educator_member() -> OrgMember {
    OrgMember {
        id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        joined_at: Utc::now(),
        grants: vec!["courses:*".to_string()],
    }
}

fn create_student_member() -> OrgMember {
    OrgMember {
        id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        joined_at: Utc::now(),
        grants: vec!["courses:read".to_string()],
    }
}

async fn educator_auth_injector(mut req: Request<Body>, next: Next) -> Response {
    let role = req.headers().get("X-Role").and_then(|v| v.to_str().ok());
    match role {
        Some("educator") => req.extensions_mut().insert(create_educator_member()),
        Some("student") => req.extensions_mut().insert(create_student_member()),
        _ => None,
    };
    next.run(req).await
}

fn build_course_router() -> Router {
    let courses = Router::new()
        .route("/courses", get(|| async { "courses-list" }).post(|| async { "course-created" }))
        .route("/courses/:id", get(|| async { "course-detail" }).patch(|| async { "course-updated" }))
        .layer(middleware::from_fn(authorizer));

    Router::new()
        .merge(courses)
        .layer(middleware::from_fn(educator_auth_injector))
}

#[tokio::test]
async fn test_educator_can_create_and_update_course() {
    let app = build_course_router();

    // POST /courses
    let req = Request::builder()
        .uri("/courses")
        .method("POST")
        .header("X-Role", "educator")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // PATCH /courses/:id
    let req = Request::builder()
        .uri("/courses/550e8400-e29b-41d4-a716-446655440000")
        .method("PATCH")
        .header("X-Role", "educator")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_student_can_read_but_not_create_or_update_course() {
    let app = build_course_router();

    // GET /courses -> 200
    let req = Request::builder()
        .uri("/courses")
        .method("GET")
        .header("X-Role", "student")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // POST /courses -> 403 Forbidden
    let req = Request::builder()
        .uri("/courses")
        .method("POST")
        .header("X-Role", "student")
        .body(Body::empty())
        .unwrap();
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    // PATCH /courses/:id -> 403 Forbidden
    let req = Request::builder()
        .uri("/courses/550e8400-e29b-41d4-a716-446655440000")
        .method("PATCH")
        .header("X-Role", "student")
        .body(Body::empty())
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
