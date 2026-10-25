use std::time::Duration;

use axum::body::Body;
use axum::extract::Path;
use axum::http::{header, Request, Response, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::get;
use axum::Router;
use jsonwebtoken::{encode, get_current_timestamp, EncodingKey, Header};
use megh::auth::http::{auth_router, MeghAuthState};
use megh::auth::{jwt_session, AccessToken, JwtSession, JWT_COOKIE};
use megh::{Member, UpsertUserInput, UserRepo};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, Session, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"jwt-session-test-secret-32-bytes!";

async fn login(session: Session, Path(user_id): Path<Uuid>) {
    session.insert("user_id", user_id).await.unwrap();
}

fn app(pool: PgPool) -> Router {
    let jwt = JwtSession::new(SECRET, Duration::from_secs(300), pool);
    let api = Router::new()
        .route("/invoices", get(|| async { "ok" }).post(|| async { "ok" }))
        .layer(from_fn(megh::authorizer))
        .layer(from_fn_with_state(jwt, jwt_session));
    api.route("/login/{user_id}", get(login)).layer(SessionManagerLayer::new(MemoryStore::default()))
}

/// A user who is a member of one organization with the `invoices:read` grant.
async fn reader(pool: &PgPool) -> (Uuid, Member) {
    let login = UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None };
    let user = UserRepo::new(pool).upsert(&login).await.unwrap();
    let org = Uuid::new_v4();
    sqlx::query("INSERT INTO organizations (id, name) VALUES ($1, 'Acme')").bind(org).execute(pool).await.unwrap();
    let member: Member = sqlx::query_as(
        "INSERT INTO organization_members (organization_id, user_id, grants) VALUES ($1, $2, '[\"invoices:read\"]') RETURNING *",
    )
    .bind(org)
    .bind(user.id)
    .fetch_one(pool)
    .await
    .unwrap();
    (user.id, member)
}

fn signed(token: &AccessToken, secret: &[u8]) -> String {
    format!("{JWT_COOKIE}={}", encode(&Header::default(), token, &EncodingKey::from_secret(secret)).unwrap())
}

async fn send(app: &Router, method: &str, uri: &str, cookie: &str) -> Response<Body> {
    let request = Request::builder().method(method).uri(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn session_cookie(app: &Router, user_id: Uuid) -> String {
    let response = send(app, "GET", &format!("/login/{user_id}"), "").await;
    response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string()
}

fn jwt_cookie(response: &Response<Body>) -> String {
    let set = response.headers().get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap()).find(|v| v.starts_with(JWT_COOKIE)).unwrap();
    assert!(set.contains("HttpOnly") && set.contains("Secure") && set.contains("SameSite=Lax") && set.contains("Max-Age=300"), "{set}");
    set.split(';').next().unwrap().to_string()
}

#[sqlx::test]
async fn a_session_is_renewed_into_a_token_that_then_answers_alone(pool: PgPool) {
    let (user_id, _) = reader(&pool).await;
    let app = app(pool.clone());
    let session = session_cookie(&app, user_id).await;

    let renewed = send(&app, "GET", "/invoices", &session).await;
    assert_eq!(renewed.status(), StatusCode::OK);
    let token = jwt_cookie(&renewed);

    pool.close().await;
    assert_eq!(send(&app, "GET", "/invoices", &token).await.status(), StatusCode::OK);
}

#[sqlx::test]
async fn the_grants_in_the_token_decide(pool: PgPool) {
    let (_, member) = reader(&pool).await;
    let app = app(pool);
    let token = signed(&AccessToken::new(&member, Duration::from_secs(60)), SECRET);

    assert_eq!(send(&app, "GET", "/invoices", &token).await.status(), StatusCode::OK);
    assert_eq!(send(&app, "POST", "/invoices", &token).await.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn an_expired_token_is_renewed_from_the_session_or_rejected(pool: PgPool) {
    let (user_id, member) = reader(&pool).await;
    let app = app(pool);
    let mut expired = AccessToken::new(&member, Duration::from_secs(60));
    expired.exp = get_current_timestamp() - 1;
    let expired = signed(&expired, SECRET);
    let session = session_cookie(&app, user_id).await;

    let renewed = send(&app, "GET", "/invoices", &format!("{expired}; {session}")).await;
    assert_eq!(renewed.status(), StatusCode::OK);
    jwt_cookie(&renewed);
    assert_eq!(send(&app, "GET", "/invoices", &expired).await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(send(&app, "GET", "/invoices", "").await.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn a_token_signed_with_another_secret_is_rejected_even_with_a_session(pool: PgPool) {
    let (user_id, member) = reader(&pool).await;
    let app = app(pool);
    let forged = signed(&AccessToken::new(&member, Duration::from_secs(60)), b"another-secret-another-secret-32b!");
    let session = session_cookie(&app, user_id).await;

    assert_eq!(send(&app, "GET", "/invoices", &format!("{forged}; {session}")).await.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn logout_clears_the_token_cookie() {
    let state = MeghAuthState::new(PgPool::connect_lazy("postgres://localhost/test").unwrap());
    let app = auth_router(state).layer(SessionManagerLayer::new(MemoryStore::default()));

    let request = Request::post("/auth/logout").header(header::COOKIE, format!("{JWT_COOKIE}=issued-earlier"));
    let response = app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap();

    let cleared = response.headers().get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap()).find(|v| v.starts_with(JWT_COOKIE)).unwrap();
    assert!(cleared.starts_with(&format!("{JWT_COOKIE}=;")) && cleared.contains("Max-Age=0") && cleared.contains("Path=/"), "{cleared}");
}

#[sqlx::test]
async fn a_deleted_users_session_cannot_renew_a_token(pool: PgPool) {
    let (user_id, _) = reader(&pool).await;
    let app = app(pool.clone());
    let session = session_cookie(&app, user_id).await;
    assert_eq!(send(&app, "GET", "/invoices", &session).await.status(), StatusCode::OK);
    sqlx::query("UPDATE users SET deleted_at = NOW() WHERE id = $1").bind(user_id).execute(&pool).await.unwrap();

    assert_eq!(send(&app, "GET", "/invoices", &session).await.status(), StatusCode::UNAUTHORIZED);
}

async fn send_with(app: &Router, method: &str, uri: &str, headers: &[(header::HeaderName, String)]) -> Response<Body> {
    let request = headers.iter().fold(Request::builder().method(method).uri(uri), |request, (name, value)| request.header(name, value));
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

fn bearer(token: &AccessToken, secret: &[u8]) -> (header::HeaderName, String) {
    (header::AUTHORIZATION, format!("Bearer {}", signed(token, secret).trim_start_matches(&format!("{JWT_COOKIE}="))))
}

#[sqlx::test]
async fn a_bearer_token_answers_alone_with_no_cookie_set(pool: PgPool) {
    let (_, member) = reader(&pool).await;
    let app = app(pool);
    let token = AccessToken::new(&member, Duration::from_secs(60));

    let read = send_with(&app, "GET", "/invoices", &[bearer(&token, SECRET)]).await;
    let write = send_with(&app, "POST", "/invoices", &[bearer(&token, SECRET)]).await;

    assert_eq!(read.status(), StatusCode::OK);
    assert!(read.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(write.status(), StatusCode::FORBIDDEN, "the grants in the bearer token decide");
}

#[sqlx::test]
async fn a_bad_or_expired_bearer_token_is_rejected_even_with_a_valid_session_and_cookie(pool: PgPool) {
    let (user_id, member) = reader(&pool).await;
    let app = app(pool);
    let session = session_cookie(&app, user_id).await;
    let valid_cookie = signed(&AccessToken::new(&member, Duration::from_secs(60)), SECRET);
    let mut expired = AccessToken::new(&member, Duration::from_secs(60));
    expired.exp = get_current_timestamp() - 1;

    for bad in [bearer(&expired, SECRET), bearer(&AccessToken::new(&member, Duration::from_secs(60)), b"another-secret-another-secret-32b!")] {
        let cookie = (header::COOKIE, format!("{valid_cookie}; {session}"));
        let response = send_with(&app, "GET", "/invoices", &[bad, cookie]).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response.headers().get(header::SET_COOKIE).is_none(), "a bearer token is never renewed");
    }
}

#[sqlx::test]
async fn an_authorization_header_that_is_not_bearer_is_ignored(pool: PgPool) {
    let (_, member) = reader(&pool).await;
    let app = app(pool);
    let cookie = (header::COOKIE, signed(&AccessToken::new(&member, Duration::from_secs(60)), SECRET));
    let basic = (header::AUTHORIZATION, "Basic YW5uQGV4YW1wbGUuY29tOnNlY3JldA==".to_string());

    assert_eq!(send_with(&app, "GET", "/invoices", &[basic, cookie]).await.status(), StatusCode::OK);
}
