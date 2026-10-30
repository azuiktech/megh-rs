//! Specification: connecting a known provider's account for a tenant (issue #234).
//!
//! - `/auth/{provider}/connect?tenant_id={t}` connects an account for tenant `t`, which the signed-in user must be a
//!   member of (`403` otherwise). Any account may be connected, such as a shared inbox; a connect without a tenant
//!   keeps the own-email check.
//! - The account is stored with the tenant and `auth` `oauth`, and is one of the tenant's connections.
//! - An account another tenant has connected is refused (`409`) and stays theirs.

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

/// The tenant sign-in gave the user holding `cookie`.
async fn own_tenant(app: &Router, cookie: &str) -> String {
    let me: Value = serde_json::from_str(&text(get_page(app, "/auth/me", cookie).await).await).unwrap();
    me["memberships"][0]["tenant_id"].as_str().unwrap().to_string()
}

async fn connect_for(app: &Router, cookie: &str, tenant: &str) -> Response<Body> {
    let start = get_page(app, &format!("/auth/stub/connect?tenant_id={tenant}"), cookie).await;
    if !start.status().is_redirection() {
        return start;
    }
    let state = params(&start)["state"].clone();
    get_page(app, &format!("/auth/stub/callback?code=abc&state={state}"), cookie).await
}

async fn stored(pool: &PgPool, account_id: &str) -> (Option<uuid::Uuid>, Option<String>, String) {
    sqlx::query_as("SELECT tenant_id, auth, email::text FROM connected_accounts WHERE account_id = $1 AND provider = 'stub'")
        .bind(account_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn a_tenant_connects_a_shared_inbox_whatever_its_email(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    let tenant = own_tenant(&app, &cookie).await;
    *stub.userinfo.lock().unwrap() = person("inbox", "orders@shop.example");

    let page = text(connect_for(&app, &cookie, &tenant).await).await;
    let listed = megh::Connections::new(pool.clone()).list(tenant.parse().unwrap()).await.unwrap();

    assert!(page.contains("oauth_connect_success"), "{page}");
    assert_eq!(stored(&pool, "inbox").await, (Some(tenant.parse().unwrap()), Some("oauth".into()), "orders@shop.example".into()));
    assert_eq!(listed.iter().map(|connection| (connection.account_id.as_str(), connection.connected)).collect::<Vec<_>>(), [("inbox", true)]);
}

#[sqlx::test]
async fn a_tenant_the_user_is_not_a_member_of_is_refused(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    let elsewhere: uuid::Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Elsewhere') RETURNING id").fetch_one(&pool).await.unwrap();
    *stub.userinfo.lock().unwrap() = person("inbox", "orders@shop.example");

    let refused = connect_for(&app, &cookie, &elsewhere.to_string()).await;
    let accounts: i64 = sqlx::query_scalar("SELECT count(*) FROM connected_accounts WHERE account_id = 'inbox'").fetch_one(&pool).await.unwrap();

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(accounts, 0);
}

#[sqlx::test]
async fn an_account_another_tenant_connected_is_refused_and_stays_theirs(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    let tenant = own_tenant(&app, &cookie).await;
    let theirs: uuid::Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Theirs') RETURNING id").fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, email, access_token, tenant_id, auth) VALUES ('inbox', 'stub', 'orders@shop.example', 'theirs', $1, 'oauth')")
        .bind(theirs)
        .execute(&pool)
        .await
        .unwrap();
    *stub.userinfo.lock().unwrap() = person("inbox", "orders@shop.example");

    let refused = connect_for(&app, &cookie, &tenant).await;
    let token: String = sqlx::query_scalar("SELECT access_token FROM connected_accounts WHERE account_id = 'inbox'").fetch_one(&pool).await.unwrap();

    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!((stored(&pool, "inbox").await.0, token.as_str()), (Some(theirs), "theirs"));
}

#[sqlx::test]
async fn a_connect_without_a_tenant_keeps_the_own_email_check(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    *stub.userinfo.lock().unwrap() = person("inbox", "orders@shop.example");

    let state = params(&get_page(&app, "/auth/stub/connect", &cookie).await)["state"].clone();
    let refused = get_page(&app, &format!("/auth/stub/callback?code=abc&state={state}"), &cookie).await;

    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn an_account_connected_again_after_its_card_was_deleted_is_back(pool: PgPool) {
    let (app, stub) = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    let tenant: uuid::Uuid = own_tenant(&app, &cookie).await.parse().unwrap();
    *stub.userinfo.lock().unwrap() = person("inbox", "orders@shop.example");
    connect_for(&app, &cookie, &tenant.to_string()).await;
    let connections = megh::Connections::new(pool.clone());
    let id = connections.list(tenant).await.unwrap()[0].id;
    connections.delete(tenant, id).await.unwrap();

    connect_for(&app, &cookie, &tenant.to_string()).await;
    let listed = connections.list(tenant).await.unwrap();

    assert_eq!(listed.iter().map(|connection| (connection.account_id.as_str(), connection.connected)).collect::<Vec<_>>(), [("inbox", true)]);
}
