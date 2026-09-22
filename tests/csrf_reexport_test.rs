//! The session-token CSRF middleware megh re-exports is usable directly on an app's own routes.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::from_fn;
use axum::routing::{get, post};
use axum::Router;
use megh::auth::{get_or_create_token, CsrfMiddleware, TOKEN_HEADER};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};

async fn token(session: Session) -> String {
    get_or_create_token(&session).await.unwrap()
}

fn app() -> Router {
    Router::new()
        .route("/orders", post(|| async { "ok" }))
        .route("/token", get(token))
        .layer(from_fn(CsrfMiddleware::middleware))
        .layer(SessionManagerLayer::new(MemoryStore::default()))
}

#[tokio::test]
async fn an_apps_own_route_is_protected_with_one_layer() {
    let app = app();
    let issued = app.clone().oneshot(Request::get("/token").body(Body::empty()).unwrap()).await.unwrap();
    let cookie = issued.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let token = String::from_utf8(axum::body::to_bytes(issued.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();

    let no_token = Request::post("/orders").header(header::COOKIE, &cookie).body(Body::empty()).unwrap();
    let with_token = Request::post("/orders").header(header::COOKIE, &cookie).header(TOKEN_HEADER, &token).body(Body::empty()).unwrap();

    assert_eq!(app.clone().oneshot(no_token).await.unwrap().status(), StatusCode::FORBIDDEN);
    assert_eq!(app.oneshot(with_token).await.unwrap().status(), StatusCode::OK);
}
