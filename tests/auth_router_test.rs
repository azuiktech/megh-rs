use axum::http::{header, Request, StatusCode};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::OAuthProviderConfig;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

fn app(state: MeghAuthState) -> axum::Router {
    auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default()))
}

#[tokio::test]
async fn test_auth_router_unconfigured_provider_returns_not_found() {
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
    let state = MeghAuthState::new(pool);
    let app = app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/github")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_auth_router_configured_provider_redirects() {
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
    let google_provider = OAuthProviderConfig::google("test-client-id", Some("secret".to_string()));
    let state = MeghAuthState::new(pool).add_provider(google_provider);
    let app = app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/google")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response
        .headers()
        .get(header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(location.starts_with("https://accounts.google.com/o/oauth2/v2/auth"));
    assert!(location.contains("client_id=test-client-id"));
}

#[tokio::test]
async fn test_auth_router_me_without_cookie_returns_unauthorized() {
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
    let state = MeghAuthState::new(pool);
    let app = app(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/auth/me")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
