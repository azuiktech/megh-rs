use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::body::{to_bytes, Body};
use axum::extract::{Form, State};
use axum::http::{header, Request, Response, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::Duration;
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::oauth::Url;
use megh::{OAuthProviderConfig, SessionRepo, UpsertUserInput, UserRepo};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;

#[derive(Clone)]
struct Idp {
    base: String,
    email: Arc<Mutex<String>>,
    verifiers: Arc<Mutex<Vec<String>>>,
    revoked: Arc<Mutex<Vec<String>>>,
}

async fn idp(email: &str) -> Idp {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let idp = Idp {
        base: format!("http://{}", listener.local_addr().unwrap()),
        email: Arc::new(Mutex::new(email.to_string())),
        verifiers: Arc::default(),
        revoked: Arc::default(),
    };
    let app = Router::new()
        .route("/token", post(|State(i): State<Idp>, Form(f): Form<HashMap<String, String>>| async move {
            i.verifiers.lock().unwrap().push(f.get("code_verifier").cloned().unwrap_or_default());
            Json(json!({"access_token": "AT", "token_type": "bearer", "expires_in": 3600, "refresh_token": "RT"}))
        }))
        .route("/userinfo", get(|State(i): State<Idp>| async move {
            Json(json!({"sub": "g-1", "email": i.email.lock().unwrap().clone(), "email_verified": true, "name": "</script><script>alert(1)</script>"}))
        }))
        .route("/revoke", post(|State(i): State<Idp>, Form(f): Form<HashMap<String, String>>| async move {
            i.revoked.lock().unwrap().push(f["token"].clone());
            StatusCode::OK
        }))
        .with_state(idp.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    idp
}

fn app(pool: &PgPool, idp: &Idp) -> Router {
    let mut google = OAuthProviderConfig::google("client-id", Some("secret".into()));
    google.auth_url = format!("{}/auth", idp.base);
    google.token_url = format!("{}/token", idp.base);
    google.userinfo_url = Some(format!("{}/userinfo", idp.base));
    google.revoke_url = Some(format!("{}/revoke", idp.base));
    auth_router(MeghAuthState::new(pool.clone()).add_provider(google))
}

async fn user_with_session(pool: &PgPool, email: &str) -> String {
    let user = UserRepo::new(pool)
        .upsert(&UpsertUserInput { provider: "google".into(), account_id: "g-1".into(), email: email.into(), display_name: None, photo_url: None })
        .await
        .unwrap();
    let session = SessionRepo::new(pool).create(user.id, Duration::days(1), "test", "", json!({})).await.unwrap();
    format!("kyrios_session={}", session.plaintext_token)
}

async fn send(app: &Router, method: &str, uri: &str, cookie: &str, body: Option<Value>) -> Response<Body> {
    let request = Request::builder().method(method).uri(uri).header(header::COOKIE, cookie);
    let request = match body {
        Some(json) => request.header(header::CONTENT_TYPE, "application/json").body(Body::from(json.to_string())),
        None => request.body(Body::empty()),
    };
    app.clone().oneshot(request.unwrap()).await.unwrap()
}

fn cookies(response: &Response<Body>) -> Vec<(String, String)> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|c| c.to_str().unwrap().split(';').next()?.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

fn cookie_header(pairs: &[(String, String)]) -> String {
    pairs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
}

fn state_of(response: &Response<Body>) -> String {
    let location = response.headers()[header::LOCATION].to_str().unwrap();
    Url::parse(location).unwrap().query_pairs().find(|(k, _)| k == "state").unwrap().1.to_string()
}

async fn text(response: Response<Body>) -> String {
    String::from_utf8(to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

async fn add_account(pool: &PgPool, account_id: &str, email: &str) {
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, email, access_token, refresh_token) VALUES ($1, 'google', $2, 'AT', 'RT')")
        .bind(account_id)
        .bind(email)
        .execute(pool)
        .await
        .unwrap();
}

async fn disconnected(pool: &PgPool, account_id: &str) -> bool {
    sqlx::query_scalar("SELECT disconnected_at IS NOT NULL FROM connected_accounts WHERE account_id = $1").bind(account_id).fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn connect_requires_a_session(pool: PgPool) {
    let app = app(&pool, &idp("ann@example.com").await);

    assert_eq!(send(&app, "GET", "/auth/google/connect", "", None).await.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn connect_starts_the_flow_with_state_pkce_consent_and_extra_scopes(pool: PgPool) {
    let app = app(&pool, &idp("ann@example.com").await);
    let session = user_with_session(&pool, "ann@example.com").await;

    let response = send(&app, "GET", "/auth/google/connect?scope=calendar&scope=drive", &session, None).await;

    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let location = response.headers()[header::LOCATION].to_str().unwrap().to_string();
    let state = state_of(&response);
    assert!(state.starts_with("connect."));
    for expected in ["code_challenge_method=S256", "prompt=consent", "calendar", "drive", "openid"] {
        assert!(location.contains(expected), "{expected} missing from {location}");
    }
    let cookies = cookies(&response);
    assert_eq!(cookies.iter().find(|(k, _)| k == "_oauth_state_google").map(|(_, v)| v.as_str()), Some(state.as_str()));
    assert!(cookies.iter().any(|(k, _)| k == "_oauth_pkce_google"));
    assert!(response.headers().get_all(header::SET_COOKIE).iter().all(|c| c.to_str().unwrap().contains("HttpOnly")));
}

#[sqlx::test]
async fn callback_rejects_a_missing_or_wrong_state_and_a_missing_pkce_verifier(pool: PgPool) {
    let app = app(&pool, &idp("ann@example.com").await);
    let login = send(&app, "GET", "/auth/google/login", "", None).await;
    let attempt = cookies(&login);
    let state = state_of(&login);

    let no_cookies = send(&app, "GET", &format!("/auth/google/callback?code=c&state={state}"), "", None).await;
    let wrong_state = send(&app, "GET", "/auth/google/callback?code=c&state=login.other", &cookie_header(&attempt), None).await;
    let state_only: Vec<_> = attempt.iter().filter(|(k, _)| k == "_oauth_state_google").cloned().collect();
    let no_pkce = send(&app, "GET", &format!("/auth/google/callback?code=c&state={state}"), &cookie_header(&state_only), None).await;

    assert_eq!((no_cookies.status(), wrong_state.status(), no_pkce.status()), (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN, StatusCode::BAD_REQUEST));
}

#[sqlx::test]
async fn login_still_signs_in_with_state_and_pkce(pool: PgPool) {
    let idp = idp("ann@example.com").await;
    let app = app(&pool, &idp);
    let login = send(&app, "GET", "/auth/google/login", "", None).await;
    let attempt = cookie_header(&cookies(&login));

    let callback = send(&app, "GET", &format!("/auth/google/callback?code=c&state={}", state_of(&login)), &attempt, None).await;

    assert_eq!(callback.status(), StatusCode::OK);
    assert!(cookies(&callback).iter().any(|(k, v)| k == "kyrios_session" && !v.is_empty()));
    assert!(text(callback).await.contains("oauth_success"));
    assert!(idp.verifiers.lock().unwrap().iter().all(|v| !v.is_empty()) && idp.verifiers.lock().unwrap().len() == 1);
}

#[sqlx::test]
async fn connect_stores_the_account_for_the_logged_in_user(pool: PgPool) {
    let app = app(&pool, &idp("ann@example.com").await);
    let session = user_with_session(&pool, "ann@example.com").await;
    let connect = send(&app, "GET", "/auth/google/connect", &session, None).await;
    let cookie = format!("{session}; {}", cookie_header(&cookies(&connect)));

    let callback = send(&app, "GET", &format!("/auth/google/callback?code=c&state={}", state_of(&connect)), &cookie, None).await;

    assert_eq!(callback.status(), StatusCode::OK);
    let page = text(callback).await;
    assert!(page.contains("oauth_connect_success") && !page.contains("\"AT\"") && !page.contains("\"RT\""));
    let stored: (String, Option<String>) = sqlx::query_as("SELECT access_token, refresh_token FROM connected_accounts WHERE account_id = 'g-1'").fetch_one(&pool).await.unwrap();
    assert_eq!(stored, ("AT".to_string(), Some("RT".to_string())));
}

#[sqlx::test]
async fn connect_refuses_an_account_with_another_email(pool: PgPool) {
    let app = app(&pool, &idp("someone.else@example.com").await);
    let session = user_with_session(&pool, "ann@example.com").await;
    let connect = send(&app, "GET", "/auth/google/connect", &session, None).await;
    let cookie = format!("{session}; {}", cookie_header(&cookies(&connect)));

    let callback = send(&app, "GET", &format!("/auth/google/callback?code=c&state={}", state_of(&connect)), &cookie, None).await;

    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM connected_accounts").fetch_one(&pool).await.unwrap();
    assert_eq!((callback.status(), stored), (StatusCode::FORBIDDEN, 0));
}

#[sqlx::test]
async fn disconnect_revokes_and_disconnects_only_the_users_own_account(pool: PgPool) {
    let idp = idp("ann@example.com").await;
    let app = app(&pool, &idp);
    let session = user_with_session(&pool, "ann@example.com").await;
    add_account(&pool, "g-1", "ann@example.com").await;
    add_account(&pool, "g-2", "bob@example.com").await;

    let others = send(&app, "POST", "/auth/google/disconnect", &session, Some(json!({"account_id": "g-2"}))).await;
    let own = send(&app, "POST", "/auth/google/disconnect", &session, Some(json!({"account_id": "g-1"}))).await;

    assert_eq!((others.status(), own.status()), (StatusCode::NOT_FOUND, StatusCode::NO_CONTENT));
    assert_eq!((disconnected(&pool, "g-1").await, disconnected(&pool, "g-2").await), (true, false));
    assert_eq!(*idp.revoked.lock().unwrap(), ["RT"]);
}

#[sqlx::test]
async fn revoke_revokes_disconnects_and_signs_out(pool: PgPool) {
    let idp = idp("ann@example.com").await;
    let app = app(&pool, &idp);
    let session = user_with_session(&pool, "ann@example.com").await;
    add_account(&pool, "g-1", "ann@example.com").await;

    let revoked = send(&app, "POST", "/auth/google/revoke", &session, None).await;
    let after = send(&app, "GET", "/auth/me", &session, None).await;

    assert_eq!((revoked.status(), after.status()), (StatusCode::NO_CONTENT, StatusCode::UNAUTHORIZED));
    assert_eq!((disconnected(&pool, "g-1").await, idp.revoked.lock().unwrap().clone()), (true, vec!["RT".to_string()]));
}

#[sqlx::test]
async fn the_callback_page_escapes_provider_supplied_text(pool: PgPool) {
    let app = app(&pool, &idp("ann@example.com").await);
    let login = send(&app, "GET", "/auth/google/login", "", None).await;
    let attempt = cookie_header(&cookies(&login));

    let page = text(send(&app, "GET", &format!("/auth/google/callback?code=c&state={}", state_of(&login)), &attempt, None).await).await;

    assert!(page.contains("oauth_success"));
    assert_eq!(page.matches("</script").count(), 1, "only the page's own closing tag may appear");
}
