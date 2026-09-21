use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::routing::post;
use axum::Router;
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::{ConfigError, CsrfLayer, OAuthProviderConfig, ProtectionError};
use tower::ServiceExt;

const WEB_ORIGIN: &str = "https://app.example.com";

fn auth_app(state: MeghAuthState) -> Router {
    auth_router(state.add_provider(OAuthProviderConfig::google("client-id", Some("secret".into()))))
}

fn default_state() -> MeghAuthState {
    MeghAuthState::new(sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap())
}

async fn send(app: &Router, method: &str, path: &str, headers: &[(&str, &str)]) -> Response<Body> {
    let request = headers
        .iter()
        .fold(Request::builder().method(method).uri(path), |b, (k, v)| b.header(*k, *v))
        .body(Body::empty())
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

fn plain_router(layer: CsrfLayer) -> Router {
    Router::new().route("/things", post(|| async { "ok" })).layer(layer)
}

#[tokio::test]
async fn cross_site_logout_is_rejected_and_has_no_effect() {
    let response = send(&auth_app(default_state()), "POST", "/auth/logout", &[("sec-fetch-site", "cross-site")]).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert!(response.extensions().get::<ProtectionError>().is_some());
}

#[tokio::test]
async fn same_origin_logout_passes() {
    let response = send(&auth_app(default_state()), "POST", "/auth/logout", &[("sec-fetch-site", "same-origin")]).await;

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get(header::SET_COOKIE).is_some());
}

#[tokio::test]
async fn non_browser_logout_passes() {
    let response = send(&auth_app(default_state()), "POST", "/auth/logout", &[]).await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn other_origin_is_rejected_by_default_and_passes_when_trusted() {
    let headers = [("sec-fetch-site", "same-site"), ("origin", WEB_ORIGIN)];

    let by_default = send(&auth_app(default_state()), "POST", "/auth/logout", &headers).await;
    assert_eq!(by_default.status(), StatusCode::FORBIDDEN);

    let trusting = default_state().with_csrf(CsrfLayer::new().add_trusted_origin(WEB_ORIGIN).unwrap());
    let trusted = send(&auth_app(trusting), "POST", "/auth/logout", &headers).await;
    assert_eq!(trusted.status(), StatusCode::OK);
}

#[tokio::test]
async fn safe_methods_are_never_checked() {
    let response = send(&auth_app(default_state()), "GET", "/auth/google", &[("sec-fetch-site", "cross-site")]).await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn bypass_predicate_exempts_a_route() {
    let bypassing = default_state()
        .with_csrf(CsrfLayer::new().with_insecure_bypass(|_, uri| uri.path() == "/auth/logout"));
    let response = send(&auth_app(bypassing), "POST", "/auth/logout", &[("sec-fetch-site", "cross-site")]).await;

    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn layer_attaches_to_any_router() {
    let app = plain_router(CsrfLayer::new());

    assert_eq!(send(&app, "POST", "/things", &[("sec-fetch-site", "cross-site")]).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&app, "POST", "/things", &[("sec-fetch-site", "same-origin")]).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn layer_attaches_to_a_single_route() {
    let app = Router::new()
        .route("/guarded", post(|| async { "ok" }).layer(CsrfLayer::new()))
        .route("/open", post(|| async { "ok" }));
    let cross_site = [("sec-fetch-site", "cross-site")];

    assert_eq!(send(&app, "POST", "/guarded", &cross_site).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(send(&app, "POST", "/open", &cross_site).await.status(), StatusCode::OK);
}

#[test]
fn malformed_trusted_origin_is_a_config_error() {
    let error = CsrfLayer::new().add_trusted_origin("https://app.example.com/").unwrap_err();

    assert!(matches!(error, ConfigError::InvalidOriginUrlComponents { .. }));
}
