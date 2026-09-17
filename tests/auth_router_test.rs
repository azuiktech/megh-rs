use axum::http::{header, Request, StatusCode};
use megh::auth::http::{auth_router, extract_cookie, MeghAuthState};
use megh::auth::OAuthProviderConfig;
use tower::ServiceExt;

#[test]
fn test_extract_cookie_utility() {
    let mut headers = axum::http::HeaderMap::new();

    // No cookie header
    assert_eq!(extract_cookie(&headers, "kyrios_session"), None);

    // Single cookie
    headers.insert(header::COOKIE, "kyrios_session=token123".parse().unwrap());
    assert_eq!(
        extract_cookie(&headers, "kyrios_session"),
        Some("token123".to_string())
    );

    // Multiple cookies
    headers.insert(
        header::COOKIE,
        "theme=dark; kyrios_session=secret_token_456; lang=en".parse().unwrap(),
    );
    assert_eq!(
        extract_cookie(&headers, "kyrios_session"),
        Some("secret_token_456".to_string())
    );

    // Different cookie requested
    assert_eq!(extract_cookie(&headers, "theme"), Some("dark".to_string()));
    assert_eq!(extract_cookie(&headers, "missing"), None);
}

#[tokio::test]
async fn test_auth_router_unconfigured_provider_returns_not_found() {
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
    let state = MeghAuthState::new(pool);
    let app = auth_router(state);

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
    let app = auth_router(state);

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
    let app = auth_router(state);

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

#[tokio::test]
async fn test_auth_router_logout_clears_cookie() {
    let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
    let state = MeghAuthState::new(pool).with_cookie_name("custom_session");
    let app = auth_router(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/auth/logout")
                .header(header::COOKIE, "custom_session=valid-or-invalid-token")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let set_cookie = response
        .headers()
        .get(header::SET_COOKIE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(set_cookie.contains("custom_session=;"));
    assert!(set_cookie.contains("Max-Age=0"));
}
