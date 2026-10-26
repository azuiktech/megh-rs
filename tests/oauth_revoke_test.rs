//! Issue #213 (#17 of the code review): megh revokes only the stored token of the signed-in user's own account. The
//! route takes the account, not a token; another user's account is not found. Sending the revocation is the `oauth2`
//! library's job and is not tested here.

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::OAuthProviderConfig;
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};

async fn token() -> Json<Value> {
    Json(json!({"access_token": "ann-token", "token_type": "bearer", "expires_in": 3600, "refresh_token": "rt"}))
}

async fn userinfo() -> Json<Value> {
    Json(json!({"sub": "1", "email": "ann@example.com", "email_verified": true, "name": "Ann"}))
}

async fn fixture(pool: PgPool) -> Router {
    let routes = Router::new().route("/token", post(token)).route("/userinfo", get(userinfo));
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
    auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default()))
}

async fn get_page(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    app.clone().oneshot(Request::get(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap()
}

fn session_of(response: &Response<Body>) -> String {
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

/// Signs Ann in through the provider, which stores her account `1`; returns her session cookie.
async fn sign_in(app: &Router) -> String {
    let start = get_page(app, "/auth/stub", "").await;
    let location = oauth2::url::Url::parse(start.headers()[header::LOCATION].to_str().unwrap()).unwrap();
    let state = location.query_pairs().find(|(key, _)| key == "state").unwrap().1.to_string();
    session_of(&get_page(app, &format!("/auth/stub/callback?code=abc&state={state}"), &session_of(&start)).await)
}

async fn post_revoke(app: &Router, cookie: &str, body: Value) -> StatusCode {
    let request = Request::post("/auth/stub/revoke")
        .header(header::COOKIE, cookie)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap().status()
}

#[sqlx::test]
async fn only_your_own_account_is_revoked(pool: PgPool) {
    let app = fixture(pool.clone()).await;
    let cookie = sign_in(&app).await;
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, email, access_token) VALUES ('9', 'stub', 'bob@example.com', 'bob-token')")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(post_revoke(&app, &cookie, json!({"account_id": "9"})).await, StatusCode::NOT_FOUND, "bob's account");
    assert!(post_revoke(&app, &cookie, json!({"token": "bob-token"})).await.is_client_error(), "a raw token is not taken");
    assert_ne!(post_revoke(&app, &cookie, json!({"account_id": "1"})).await, StatusCode::NOT_FOUND, "ann's own account goes on to the provider");
}
