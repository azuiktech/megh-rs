//! The CSRF check is for requests that sign in by cookie; a request with a bearer token is not checked.

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::post;
use axum::Router;
use chrono::Utc;
use jsonwebtoken::{encode, EncodingKey, Header};
use megh::auth::{csrf_unless_bearer, jwt_session, AccessToken, JwtSession, JWT_COOKIE};
use megh::Member;
use tower::ServiceExt;
use tower_http::csrf::CsrfLayer;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"csrf-bearer-test-secret-32-bytes!";

fn app() -> Router {
    let jwt = JwtSession::new(SECRET, Duration::from_secs(300), sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap());
    Router::new()
        .route("/invoices", post(|| async { "ok" }))
        .layer(from_fn(megh::authorizer))
        .layer(from_fn_with_state(jwt, jwt_session))
        .route_layer(from_fn_with_state(CsrfLayer::new(), csrf_unless_bearer))
        .layer(SessionManagerLayer::new(MemoryStore::default()))
}

fn token() -> String {
    let member = Member {
        id: Uuid::now_v7(),
        tenant_id: Uuid::now_v7(),
        user_id: Uuid::now_v7(),
        role: "machine".into(),
        grants: vec!["invoices:create".into()],
        joined_at: Utc::now(),
        invited_by: None,
    };
    encode(&Header::default(), &AccessToken::new(&member, Duration::from_secs(60)), &EncodingKey::from_secret(SECRET)).unwrap()
}

async fn write(auth: (header::HeaderName, String), site: Option<&str>) -> StatusCode {
    write_with(&[auth], site).await
}

async fn write_with(headers: &[(header::HeaderName, String)], site: Option<&str>) -> StatusCode {
    let request = headers.iter().fold(Request::post("/invoices"), |request, (name, value)| request.header(name, value));
    let request = site.iter().fold(request, |request, site| request.header("sec-fetch-site", *site));
    app().oneshot(request.body(Body::empty()).unwrap()).await.unwrap().status()
}

fn bearer() -> (header::HeaderName, String) {
    (header::AUTHORIZATION, format!("Bearer {}", token()))
}

fn cookie() -> (header::HeaderName, String) {
    (header::COOKIE, format!("{JWT_COOKIE}={}", token()))
}

#[tokio::test]
async fn a_machine_without_browser_headers_writes_with_its_bearer_token() {
    assert_eq!(write(bearer(), None).await, StatusCode::OK);
}

#[tokio::test]
async fn a_page_on_another_site_writes_with_a_bearer_token() {
    assert_eq!(write(bearer(), Some("cross-site")).await, StatusCode::OK);
}

#[tokio::test]
async fn a_cookie_request_is_still_checked() {
    assert_eq!(write(cookie(), Some("cross-site")).await, StatusCode::FORBIDDEN);
    assert_eq!(write(cookie(), Some("same-origin")).await, StatusCode::OK);
}

#[tokio::test]
async fn a_bearer_header_cannot_carry_a_cookie_request_past_the_check() {
    let smuggled = [cookie(), (header::AUTHORIZATION, "Bearer not-a-token".to_string())];

    assert_eq!(write_with(&smuggled, Some("cross-site")).await, StatusCode::UNAUTHORIZED);
}
