//! The OAuth login and callback against a stub provider: state, PKCE, verified email, and the result page.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::State;
use axum::http::{header, Request, Response, StatusCode};
use axum::routing::{get, post};
use axum::{Form, Json, Router};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::{OAuthProviderConfig, PkceCodeChallenge, PkceCodeVerifier};
use megh::Encryptor;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

#[derive(Clone)]
struct Stub {
    userinfo: Value,
    fail_token: bool,
    token_requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

async fn token(State(stub): State<Stub>, Form(form): Form<HashMap<String, String>>) -> (StatusCode, Json<Value>) {
    stub.token_requests.lock().unwrap().push(form);
    match stub.fail_token {
        true => (StatusCode::BAD_REQUEST, Json(json!({"error": "secret-detail-xyz"}))),
        false => (StatusCode::OK, Json(json!({"access_token": "at", "token_type": "bearer", "expires_in": 3600, "refresh_token": "rt"}))),
    }
}

async fn userinfo(State(stub): State<Stub>) -> Json<Value> {
    Json(stub.userinfo)
}

async fn spawn_stub(stub: Stub) -> String {
    let app = Router::new().route("/token", post(token)).route("/userinfo", get(userinfo)).with_state(stub);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

struct Fixture {
    app: Router,
    requests: Arc<Mutex<Vec<HashMap<String, String>>>>,
}

async fn fixture(pool: PgPool, userinfo: Value, fail_token: bool, web_origin: Option<&str>) -> Fixture {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let base = spawn_stub(Stub { userinfo, fail_token, token_requests: requests.clone() }).await;
    let provider = OAuthProviderConfig {
        provider_id: "stub".into(),
        client_id: "client".into(),
        client_secret: Some("secret".into()),
        auth_url: format!("{base}/authorize"),
        token_url: format!("{base}/token"),
        userinfo_url: Some(format!("{base}/userinfo")),
        default_scopes: vec![],
        redirect_url: None,
        revoke_url: None,
    };
    let state = MeghAuthState::new(pool).add_provider(provider).with_app_origin("http://api.test");
    let state = match web_origin {
        Some(origin) => state.with_web_origin(origin),
        None => state,
    };
    let state = state.with_token_encryptor(Encryptor::new(&[5u8; 32]));
    Fixture { app: auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default())), requests }
}

fn ann() -> Value {
    json!({"sub": "1", "email": "ann@example.com", "email_verified": true, "name": "Ann"})
}

async fn get_page(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    app.clone().oneshot(Request::get(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap()
}

async fn body(response: Response<Body>) -> String {
    String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

/// Starts a login: the session cookie, and the `state` and `code_challenge` sent to the provider.
async fn start_login(app: &Router) -> (String, String, String) {
    let response = get_page(app, "/auth/stub", "").await;
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let location = oauth2::url::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let param = |name: &str| location.query_pairs().find(|(key, _)| key == name).map(|(_, value)| value.to_string()).unwrap_or_default();
    assert_eq!(param("code_challenge_method"), "S256");
    (cookie, param("state"), param("code_challenge"))
}

async fn users(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn the_provider_is_sent_a_random_state_and_a_pkce_challenge(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, ann(), false, None).await;

    let (_, first_state, challenge) = start_login(&app).await;
    let (_, second_state, _) = start_login(&app).await;

    assert!(!first_state.is_empty() && !challenge.is_empty());
    assert_ne!(first_state, second_state);
}

#[sqlx::test]
async fn a_login_that_was_started_completes_with_its_pkce_verifier(pool: PgPool) {
    let Fixture { app, requests } = fixture(pool.clone(), ann(), false, None).await;
    let (cookie, state, challenge) = start_login(&app).await;

    let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!(response.status(), StatusCode::OK);
    let logged_in = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    assert_ne!(logged_in, cookie, "the session id is renewed at login");
    let sent = requests.lock().unwrap()[0].clone();
    let verifier = PkceCodeVerifier::new(sent["code_verifier"].clone());
    assert_eq!(PkceCodeChallenge::from_code_verifier_sha256(&verifier).as_str(), challenge);
    let me = get_page(&app, "/auth/me", &logged_in).await;
    assert_eq!(me.status(), StatusCode::OK);
    assert!(body(me).await.contains("ann@example.com"));
    let saved: i64 = sqlx::query_scalar("SELECT count(*) FROM connected_accounts").fetch_one(&pool).await.unwrap();
    assert_eq!(saved, 1);
}

#[sqlx::test]
async fn a_wrong_state_is_rejected_before_the_provider_is_called(pool: PgPool) {
    let Fixture { app, requests } = fixture(pool.clone(), ann(), false, None).await;
    let (cookie, _, _) = start_login(&app).await;

    let response = get_page(&app, "/auth/stub/callback?code=abc&state=forged", &cookie).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!((get_page(&app, "/auth/me", &cookie).await.status(), users(&pool).await), (StatusCode::UNAUTHORIZED, 0));
}

#[sqlx::test]
async fn a_callback_without_a_started_login_is_rejected(pool: PgPool) {
    let Fixture { app, requests } = fixture(pool, ann(), false, None).await;

    let no_session = get_page(&app, "/auth/stub/callback?code=abc&state=anything", "").await;
    let no_state = get_page(&app, "/auth/stub/callback?code=abc", "").await;

    assert_eq!((no_session.status(), no_state.status()), (StatusCode::BAD_REQUEST, StatusCode::BAD_REQUEST));
    assert!(requests.lock().unwrap().is_empty());
}

#[sqlx::test]
async fn a_login_can_be_answered_only_once_even_by_a_wrong_state(pool: PgPool) {
    let Fixture { app, requests } = fixture(pool, ann(), false, None).await;
    let (cookie, state, _) = start_login(&app).await;

    let guess = get_page(&app, "/auth/stub/callback?code=abc&state=guess", &cookie).await;
    let right_state_afterwards = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!((guess.status(), right_state_afterwards.status()), (StatusCode::FORBIDDEN, StatusCode::BAD_REQUEST));
    assert!(requests.lock().unwrap().is_empty());
}

#[sqlx::test]
async fn an_email_the_provider_did_not_verify_is_rejected(pool: PgPool) {
    for userinfo in [json!({"sub": "1", "email": "ann@example.com", "email_verified": false}), json!({"sub": "1", "email": "ann@example.com"})] {
        let Fixture { app, .. } = fixture(pool.clone(), userinfo, false, None).await;
        let (cookie, state, _) = start_login(&app).await;

        let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!((get_page(&app, "/auth/me", &cookie).await.status(), users(&pool).await), (StatusCode::UNAUTHORIZED, 0));
    }
}

#[sqlx::test]
async fn the_result_page_posts_to_the_web_origin_and_escapes_the_profile(pool: PgPool) {
    let hostile = json!({"sub": "1", "email": "ann@example.com", "email_verified": true, "name": "</script><script>alert(1)</script>"});
    let Fixture { app, .. } = fixture(pool, hostile, false, Some("https://web.example.com")).await;
    let (cookie, state, _) = start_login(&app).await;

    let page = body(get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await).await;

    assert!(page.contains(r#"data-target="https://web.example.com""#));
    assert!(!page.contains(r#","*")"#) && !page.contains("<script>alert"));
}

#[sqlx::test]
async fn provider_failures_do_not_reach_the_client(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool, ann(), true, None).await;
    let (cookie, state, _) = start_login(&app).await;

    let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert!(!body(response).await.contains("secret-detail-xyz"));
}

#[sqlx::test]
async fn a_configured_token_encryptor_encrypts_what_the_callback_saves(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), ann(), false, None).await;
    let (cookie, state, _) = start_login(&app).await;

    let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!(response.status(), StatusCode::OK);
    let stored: (String, String) = sqlx::query_as("SELECT access_token, refresh_token FROM connected_accounts").fetch_one(&pool).await.unwrap();
    assert!(stored.0 != "at" && Encryptor::new(&[5u8; 32]).decrypt(&stored.0).unwrap() == "at");
    assert_eq!(Encryptor::new(&[5u8; 32]).decrypt(&stored.1).unwrap(), "rt");
}

#[sqlx::test]
async fn a_deleted_user_cannot_sign_in_through_the_provider(pool: PgPool) {
    sqlx::query("INSERT INTO users (provider, account_id, email, deleted_at) VALUES ('stub', '1', 'ann@example.com', NOW())").execute(&pool).await.unwrap();
    let Fixture { app, .. } = fixture(pool.clone(), ann(), false, None).await;
    let (cookie, state, _) = start_login(&app).await;

    let response = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(get_page(&app, "/auth/me", &cookie).await.status(), StatusCode::UNAUTHORIZED);
}

async fn tenants(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM tenants").fetch_one(pool).await.unwrap()
}

async fn me(app: &Router, cookie: &str) -> Value {
    serde_json::from_str(&body(get_page(app, "/auth/me", cookie).await).await).unwrap()
}

/// Completes a login and answers the signed-in session's cookie.
async fn log_in(app: &Router) -> String {
    let (cookie, state, _) = start_login(app).await;
    let response = get_page(app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

#[sqlx::test]
async fn a_first_login_creates_a_tenant_owned_by_the_user_with_every_grant(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), ann(), false, None).await;

    let cookie = log_in(&app).await;

    assert_eq!(tenants(&pool).await, 1);
    let me: Value = serde_json::from_str(&body(get_page(&app, "/auth/me", &cookie).await).await).unwrap();
    let memberships = me["memberships"].as_array().unwrap();
    assert_eq!(memberships.len(), 1);
    assert_eq!((memberships[0]["role"].as_str(), memberships[0]["grants"].clone()), (Some("owner"), json!(["*"])));
    assert_eq!(memberships[0]["user_id"], me["user"]["id"]);
    let named: Option<String> = sqlx::query_scalar("SELECT name FROM tenants").fetch_one(&pool).await.unwrap();
    assert_eq!(named.as_deref(), Some("ann@example.com"));
}

#[sqlx::test]
async fn a_later_login_keeps_the_users_tenant(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), ann(), false, None).await;
    let first = log_in(&app).await;
    let tenant = me(&app, &first).await["memberships"][0]["tenant_id"].clone();

    let second = log_in(&app).await;

    assert_eq!(tenants(&pool).await, 1);
    assert_eq!(me(&app, &second).await["memberships"][0]["tenant_id"], tenant);
}

#[sqlx::test]
async fn a_user_who_already_has_a_membership_gets_no_new_tenant(pool: PgPool) {
    let Fixture { app, .. } = fixture(pool.clone(), ann(), false, None).await;
    let user: uuid::Uuid = sqlx::query_scalar("INSERT INTO users (id, email) VALUES ($1, 'ann@example.com') RETURNING id")
        .bind(uuid::Uuid::now_v7())
        .fetch_one(&pool)
        .await
        .unwrap();
    let acme: uuid::Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Acme') RETURNING id").fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO tenant_members (id, tenant_id, user_id, grants) VALUES ($1, $2, $3, '[\"invoices:read\"]')")
        .bind(uuid::Uuid::now_v7())
        .bind(acme)
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();

    let cookie = log_in(&app).await;

    assert_eq!(tenants(&pool).await, 1);
    let me: Value = serde_json::from_str(&body(get_page(&app, "/auth/me", &cookie).await).await).unwrap();
    assert_eq!(me["memberships"][0]["tenant_id"], json!(acme));
}
