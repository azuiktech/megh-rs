use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::CsrfLayer;
use tower::ServiceExt;

const WEB_ORIGIN: &str = "https://app.example.com";

fn state() -> MeghAuthState {
    MeghAuthState::new(sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap())
}

async fn logout_from_web_app(state: MeghAuthState) -> Response<Body> {
    let request = Request::builder()
        .method("POST")
        .uri("/auth/logout")
        .header("sec-fetch-site", "same-site")
        .header("origin", WEB_ORIGIN)
        .body(Body::empty())
        .unwrap();
    auth_router(state).oneshot(request).await.unwrap()
}

#[tokio::test]
async fn auth_router_is_csrf_protected_by_default() {
    let response = logout_from_web_app(state()).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn with_csrf_configures_auth_router() {
    let state = state().with_csrf(CsrfLayer::new().add_trusted_origin(WEB_ORIGIN).unwrap());

    assert_eq!(logout_from_web_app(state).await.status(), StatusCode::OK);
}
