use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use megh::auth::http::{auth_router, auth_router_with_csrf, MeghAuthState};
use tower::ServiceExt;
use tower_http::csrf::CsrfLayer;
use tower_sessions::{MemoryStore, SessionManagerLayer};

fn state() -> MeghAuthState {
    MeghAuthState::new(sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap())
}

fn session_layer() -> SessionManagerLayer<MemoryStore> {
    SessionManagerLayer::new(MemoryStore::default())
}

async fn logout(app: &axum::Router, headers: &[(&str, &str)]) -> Response<Body> {
    let request = headers.iter().fold(Request::post("/auth/logout"), |request, (name, value)| request.header(*name, *value));
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

#[tokio::test]
async fn a_cross_site_browser_write_is_rejected() {
    let app = auth_router(state()).layer(session_layer());

    assert_eq!(logout(&app, &[("sec-fetch-site", "cross-site")]).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(logout(&app, &[("sec-fetch-site", "same-site"), ("origin", "https://xyz.com"), ("host", "api.xyz.com")]).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_same_origin_browser_write_and_a_server_write_are_allowed() {
    let app = auth_router(state()).layer(session_layer());

    assert_eq!(logout(&app, &[("sec-fetch-site", "same-origin")]).await.status(), StatusCode::OK);
    assert_eq!(logout(&app, &[("sec-fetch-site", "none")]).await.status(), StatusCode::OK);
    assert_eq!(logout(&app, &[]).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn a_trusted_origin_may_write_from_a_subdomain() {
    let csrf = CsrfLayer::new().add_trusted_origin("https://xyz.com").unwrap();
    let app = auth_router_with_csrf(state(), csrf).layer(session_layer());

    assert_eq!(logout(&app, &[("sec-fetch-site", "same-site"), ("origin", "https://xyz.com")]).await.status(), StatusCode::OK);
    assert_eq!(logout(&app, &[("sec-fetch-site", "cross-site"), ("origin", "https://evil.com")]).await.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn no_csrf_token_is_issued_any_more() {
    let app = auth_router(state()).layer(session_layer());

    let response = app.oneshot(Request::get("/auth/csrf-token").body(Body::empty()).unwrap()).await.unwrap();

    assert!(!response.status().is_success(), "{}", response.status());
}
