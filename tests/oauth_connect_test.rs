//! Adding, disconnecting and revoking an account of a signed-in user, against a stub provider.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, Response, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::OAuthProviderConfig;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

#[derive(Clone, Default)]
struct Stub {
    userinfo: Arc<Mutex<Value>>,
}

async fn token() -> Json<Value> {
    Json(json!({"access_token": "at", "token_type": "bearer", "expires_in": 3600, "refresh_token": "rt"}))
}

async fn userinfo(State(stub): State<Stub>) -> Json<Value> {
    Json(stub.userinfo.lock().unwrap().clone())
}

fn person(sub: &str, email: &str) -> Value {
    json!({"sub": sub, "email": email, "email_verified": true, "name": "N"})
}

async fn fixture(pool: PgPool) -> (Router, Stub) {
    let stub = Stub { userinfo: Arc::new(Mutex::new(person("1", "ann@example.com"))) };
    let routes = Router::new().route("/token", post(token)).route("/userinfo", get(userinfo)).with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, routes).await.unwrap() });
    let provider = OAuthProviderConfig {
        provider_id: "stub".into(),
        client_id: "client".into(),
        client_secret: Some("secret".into()),
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        userinfo_url: Some(format!("{base}/userinfo")),
        default_scopes: vec!["openid".into()],
        redirect_url: None,
        revoke_url: None,
    };
    let state = MeghAuthState::new(pool).add_provider(provider).with_app_origin("http://api.test");
    (auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default())), stub)
}

async fn send(app: &Router, request: Request<Body>) -> Response<Body> {
    app.clone().oneshot(request).await.unwrap()
}

async fn get_page(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    send(app, Request::get(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await
}

async fn text(response: Response<Body>) -> String {
    String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

fn session_of(response: &Response<Body>) -> String {
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

fn params(response: &Response<Body>) -> HashMap<String, String> {
    let location = oauth2::url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    location.query_pairs().map(|(key, value)| (key.to_string(), value.to_string())).collect()
}

/// Signs Ann in through the provider; returns her session cookie.
async fn sign_in(app: &Router) -> String {
    let start = get_page(app, "/auth/stub", "").await;
    let (cookie, state) = (session_of(&start), params(&start)["state"].clone());
    session_of(&get_page(app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await)
}

/// Connects an account through the provider as the user holding `cookie`.
async fn connect(app: &Router, cookie: &str) -> Response<Body> {
    let state = params(&get_page(app, "/auth/stub/connect", cookie).await)["state"].clone();
    get_page(app, &format!("/auth/stub/callback?code=abc&state={state}"), cookie).await
}

async fn post_json(app: &Router, uri: &str, cookie: &str, body: Value) -> Response<Body> {
    let request = Request::post(uri)
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    send(app, request).await
}

async fn active_accounts(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM connected_accounts WHERE disconnected_at IS NULL").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn connecting_needs_a_signed_in_user(pool: PgPool) {
    let (app, _) = fixture(pool).await;

    assert_eq!(get_page(&app, "/auth/stub/connect", "").await.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn connecting_adds_an_account_and_leaves_the_session_alone(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    *stub.userinfo.lock().unwrap() = person("2", "ann@example.com");

    let start = get_page(&app, "/auth/stub/connect?scope=calendar&scope=drive", &cookie).await;
    let sent = params(&start);
    assert!(sent["state"].starts_with("connect."));
    assert_eq!(sent["scope"], "openid calendar drive");
    assert_eq!((sent["access_type"].as_str(), sent["prompt"].as_str()), ("offline", "consent"));
    let page = text(get_page(&app, &format!("/auth/stub/callback?code=abc&state={}", sent["state"]), &cookie).await).await;

    assert!(page.contains("oauth_connect_success"));
    assert!(!page.contains("access_token") && !page.contains("refresh_token"), "the page names the account, never a token");
    assert_eq!(active_accounts(&pool).await, 2);
    assert_eq!(get_page(&app, "/auth/me", &cookie).await.status(), StatusCode::OK);
}

#[sqlx::test]
async fn an_account_with_someone_elses_email_is_refused(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    *stub.userinfo.lock().unwrap() = person("2", "bob@example.com");

    assert_eq!(connect(&app, &cookie).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(active_accounts(&pool).await, 1);
}

#[sqlx::test]
async fn disconnecting_marks_the_account_and_only_your_own(pool: PgPool) {
    let (app, _) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, email, access_token) VALUES ('9', 'stub', 'bob@example.com', 'bobs')")
        .execute(&pool)
        .await
        .unwrap();

    let theirs = post_json(&app, "/auth/stub/disconnect", &cookie, json!({"account_id": "9"})).await;
    let mine = post_json(&app, "/auth/stub/disconnect", &cookie, json!({"account_id": "1"})).await;

    assert_eq!((theirs.status(), mine.status()), (StatusCode::NOT_FOUND, StatusCode::NO_CONTENT));
    assert_eq!(active_accounts(&pool).await, 1, "bob's account is still connected");
}

#[sqlx::test]
async fn a_sign_in_that_is_not_in_a_popup_returns_only_to_a_page_of_ours(pool: PgPool) {
    let (app, _) = fixture(pool).await;

    for (return_to, lands_on) in [("http%3A%2F%2Fapi.test%2Ftimetable", "http://api.test/timetable"), ("http%3A%2F%2Fevil.test%2Fx", "/")] {
        let start = get_page(&app, &format!("/auth/stub?return_to={return_to}"), "").await;
        let (cookie, state) = (session_of(&start), params(&start)["state"].clone());

        let page = text(get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await).await;

        assert!(page.contains(&format!(r#"data-fallback="{lands_on}""#)), "{return_to} landed elsewhere");
    }
}
