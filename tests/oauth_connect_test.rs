//! Connect an extra account, disconnect it, and revoke it at the provider. Every route requires login, and an
//! account can only be touched by the user it belongs to (linked by email) — unlike megh-go (review G1).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, Response, StatusCode};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::OAuthProviderConfig;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

#[derive(Clone)]
struct Stub {
    revoke_calls: Arc<Mutex<Vec<String>>>,
    fail_revoke: bool,
}

async fn token(State(_): State<Stub>, Form(_): Form<HashMap<String, String>>) -> Json<Value> {
    Json(json!({"access_token": "at", "token_type": "bearer", "expires_in": 3600, "refresh_token": "rt"}))
}

async fn userinfo(email: &'static str) -> Json<Value> {
    Json(json!({"sub": "1", "email": email, "email_verified": true, "name": "Ann"}))
}

async fn revoke(State(stub): State<Stub>, Form(form): Form<HashMap<String, String>>) -> StatusCode {
    stub.revoke_calls.lock().unwrap().push(form.get("token").cloned().unwrap_or_default());
    match stub.fail_revoke {
        true => StatusCode::BAD_REQUEST,
        false => StatusCode::OK,
    }
}

struct Fixture {
    app: Router,
    revoke_calls: Arc<Mutex<Vec<String>>>,
}

async fn fixture(pool: PgPool, fail_revoke: bool) -> Fixture {
    let revoke_calls = Arc::new(Mutex::new(Vec::new()));
    let stub = Stub { revoke_calls: revoke_calls.clone(), fail_revoke };
    let app = Router::new()
        .route("/token", post(token))
        .route("/userinfo", get(|| userinfo("ann@example.com")))
        .route("/revoke", post(revoke))
        .with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let provider = OAuthProviderConfig {
        provider_id: "stub".into(),
        client_id: "client".into(),
        client_secret: Some("secret".into()),
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        userinfo_url: Some(format!("{base}/userinfo")),
        default_scopes: vec![],
        redirect_url: None,
        revoke_url: Some(format!("{base}/revoke")),
    };
    let state = MeghAuthState::new(pool).add_provider(provider);
    Fixture { app: auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default())), revoke_calls }
}

async fn get_page(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    let mut request = Request::get(uri);
    if !cookie.is_empty() {
        request = request.header(header::COOKIE, cookie);
    }
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

/// A fresh session with no signed-in user: its own cookie, so CSRF checks pass consistently across requests.
async fn anonymous_session(app: &Router) -> String {
    let response = get_page(app, "/auth/csrf-token", "").await;
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

/// The session's CSRF token, fetched with its own cookie (a GET, so it needs no token itself).
async fn csrf_token(app: &Router, cookie: &str) -> String {
    let response = get_page(app, "/auth/csrf-token", cookie).await;
    String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

async fn post_json(app: &Router, uri: &str, cookie: &str, body: Value) -> Response<Body> {
    let token = csrf_token(app, cookie).await;
    let request = Request::post(uri).header(header::COOKIE, cookie).header(header::CONTENT_TYPE, "application/json").header("x-csrf-token", token);
    app.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap()
}

/// Logs a user in through the stub provider; returns their session cookie.
async fn logged_in(app: &Router) -> String {
    let start = get_page(app, "/auth/stub", "").await;
    let cookie = start.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let location = oauth2::url::Url::parse(start.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = location.query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
    let callback = get_page(app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;
    callback.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

#[sqlx::test]
async fn connect_requires_login(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, false).await;

    let response = get_page(&app, "/auth/stub/connect", "").await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn connect_sends_extra_scopes_and_forces_consent(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, false).await;
    let cookie = logged_in(&app).await;

    let response = get_page(&app, "/auth/stub/connect?scope=drive&scope=gmail", &cookie).await;

    let location = oauth2::url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let scope = location.query_pairs().find(|(k, _)| k == "scope").unwrap().1.to_string();
    assert_eq!((scope.split_whitespace().collect::<Vec<_>>(), location.query_pairs().find(|(k, _)| k == "prompt").unwrap().1), (vec!["drive", "gmail"], "consent".into()));
}

#[sqlx::test]
async fn connect_callback_links_the_account_without_creating_a_new_user(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), false).await;
    let cookie = logged_in(&app).await;
    let connect = get_page(&app, "/auth/stub/connect", &cookie).await;
    let state = oauth2::url::Url::parse(connect.headers()[header::LOCATION].to_str().unwrap()).unwrap().query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();

    let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!(response.status(), StatusCode::OK);
    let page = String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    assert!(page.contains("oauth_connect_success"));
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!(users, 1);
}

#[sqlx::test]
async fn connect_callback_needs_a_logged_in_session(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, false).await;
    let cookie = logged_in(&app).await;
    let connect = get_page(&app, "/auth/stub/connect", &cookie).await;
    let state = oauth2::url::Url::parse(connect.headers()[header::LOCATION].to_str().unwrap()).unwrap().query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string();
    post_json(&app, "/auth/logout", &cookie, json!({})).await;

    let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    // logout flushes the whole session, so the pending connect attempt is gone too: "no login in progress",
    // not the deeper "not signed in" check inside the connect branch, which that leaves unreachable here.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn disconnect_marks_the_users_own_account_disconnected(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), false).await;
    let ann = logged_in(&app).await;

    let response = post_json(&app, "/auth/stub/disconnect", &ann, json!({"account_id": "1"})).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let disconnected: bool = sqlx::query_scalar("SELECT disconnected_at IS NOT NULL FROM connected_accounts WHERE account_id = '1'").fetch_one(&pool).await.unwrap();
    assert!(disconnected);
}

#[sqlx::test]
async fn disconnecting_an_account_that_is_not_yours_is_not_found(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), false).await;
    let ann = logged_in(&app).await;
    sqlx::query("UPDATE connected_accounts SET email = 'someone-else@example.com'").execute(&pool).await.unwrap();

    let response = post_json(&app, "/auth/stub/disconnect", &ann, json!({"account_id": "1"})).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn revoke_calls_the_provider_disconnects_and_signs_out(pool: PgPool) {
    let Fixture { app, revoke_calls } = fixture(pool.clone(), false).await;
    let cookie = logged_in(&app).await;

    let response = post_json(&app, "/auth/stub/revoke", &cookie, json!({})).await;

    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(revoke_calls.lock().unwrap().as_slice(), ["at"]);
    let disconnected: bool = sqlx::query_scalar("SELECT disconnected_at IS NOT NULL FROM connected_accounts").fetch_one(&pool).await.unwrap();
    assert!(disconnected);
    let logged_out = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    assert_eq!(get_page(&app, "/auth/me", &logged_out).await.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_failed_provider_revoke_leaves_the_account_connected_and_the_session_active(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), true).await;
    let cookie = logged_in(&app).await;

    let response = post_json(&app, "/auth/stub/revoke", &cookie, json!({})).await;

    assert_ne!(response.status(), StatusCode::NO_CONTENT);
    let disconnected: bool = sqlx::query_scalar("SELECT disconnected_at IS NOT NULL FROM connected_accounts").fetch_one(&pool).await.unwrap();
    assert!(!disconnected);
    assert_eq!(get_page(&app, "/auth/me", &cookie).await.status(), StatusCode::OK);
}

#[sqlx::test]
async fn revoke_without_a_configured_revoke_url_is_rejected(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, false).await;
    let cookie = logged_in(&app).await;

    let response = post_json(&app, "/auth/other/revoke", &cookie, json!({})).await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn every_route_needs_a_session(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, false).await;
    let anon = anonymous_session(&app).await;

    assert_eq!(post_json(&app, "/auth/stub/disconnect", &anon, json!({"account_id": "1"})).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(post_json(&app, "/auth/stub/revoke", &anon, json!({})).await.status(), StatusCode::UNAUTHORIZED);
}
