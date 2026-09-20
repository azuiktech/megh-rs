//! Integration tests for generic route-based Authorizer middleware.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::{delete, get};
use axum::Router;
use chrono::Utc;
use tower::ServiceExt;
use uuid::Uuid;

use megh::{authorizer, OrgMember};

fn create_test_member() -> OrgMember {
    OrgMember {
        id: Uuid::new_v4(),
        organization_id: Uuid::new_v4(),
        user_id: Uuid::new_v4(),
        joined_at: Utc::now(),
        grants: vec![
            "invoices:read".to_string(),
            "members:delete:mem-1".to_string(),
        ],
    }
}

async fn fake_auth_injector(mut req: Request<Body>, next: Next) -> Response {
    if req.headers().get("X-Test-Auth").is_some() {
        req.extensions_mut().insert(create_test_member());
    }
    next.run(req).await
}

fn build_test_router() -> Router {
    let api = Router::new()
        .route("/invoices", get(|| async { "ok-invoices" }).post(|| async { "ok-create-invoice" }))
        .route("/invoices/:id", get(|| async { "ok-invoice-detail" }))
        .route("/members/:id", delete(|| async { "ok-delete-member" }))
        .layer(middleware::from_fn(authorizer));

    Router::new()
        .nest("/api", api)
        .layer(middleware::from_fn(fake_auth_injector))
}

#[tokio::test]
async fn test_authorizer_unauthenticated_returns_401() {
    let app = build_test_router();
    let req = Request::builder()
        .uri("/api/invoices")
        .method("GET")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_authorizer_authorized_read_returns_200() {
    let app = build_test_router();
    let req = Request::builder()
        .uri("/api/invoices")
        .method("GET")
        .header("X-Test-Auth", "true")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_authorizer_authorized_instance_read_returns_200() {
    let app = build_test_router();
    let req = Request::builder()
        .uri("/api/invoices/99")
        .method("GET")
        .header("X-Test-Auth", "true")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_authorizer_unauthorized_post_returns_403() {
    let app = build_test_router();
    let req = Request::builder()
        .uri("/api/invoices")
        .method("POST")
        .header("X-Test-Auth", "true")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_authorizer_authorized_instance_delete_returns_200() {
    let app = build_test_router();
    let req = Request::builder()
        .uri("/api/members/mem-1")
        .method("DELETE")
        .header("X-Test-Auth", "true")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_authorizer_unauthorized_instance_delete_returns_403() {
    let app = build_test_router();
    let req = Request::builder()
        .uri("/api/members/mem-2")
        .method("DELETE")
        .header("X-Test-Auth", "true")
        .body(Body::empty())
        .unwrap();

    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
