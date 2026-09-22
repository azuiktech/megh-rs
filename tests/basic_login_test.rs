use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use megh::auth::http::{auth_router, basic_login_router, MeghAuthState};
use megh::{Org, UserRepo};
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

fn app(pool: PgPool) -> axum::Router {
    let state = MeghAuthState::new(pool);
    auth_router(state.clone()).merge(basic_login_router(state, "/auth/login")).layer(SessionManagerLayer::new(MemoryStore::default()))
}

fn basic(email: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{email}:{password}")))
}

async fn csrf_token(app: &axum::Router) -> (String, String) {
    let response = app.clone().oneshot(Request::get("/auth/csrf-token").body(Body::empty()).unwrap()).await.unwrap();
    let cookie = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let token = String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap();
    (cookie, token)
}

async fn login(app: &axum::Router, cookie: &str, token: &str, auth: Option<&str>) -> axum::http::Response<Body> {
    let mut request = Request::post("/auth/login").header(header::COOKIE, cookie).header("x-csrf-token", token);
    if let Some(auth) = auth {
        request = request.header(header::AUTHORIZATION, auth);
    }
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

async fn ann_with_password(pool: &PgPool) -> Uuid {
    let repo = UserRepo::new(pool);
    let user = repo
        .upsert(&megh::UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None })
        .await
        .unwrap();
    repo.set_password(user.id, "s3cret-pass").await.unwrap();
    user.id
}

#[sqlx::test]
async fn correct_credentials_log_in_and_return_memberships(pool: PgPool) {
    let user_id = ann_with_password(&pool).await;
    let org: Org = sqlx::query_as("INSERT INTO organizations (name) VALUES ('Acme') RETURNING *").fetch_one(&pool).await.unwrap();
    sqlx::query("INSERT INTO organization_members (organization_id, user_id, grants) VALUES ($1, $2, '[\"invoices:read\"]')").bind(org.id).bind(user_id).execute(&pool).await.unwrap();
    let app = app(pool);
    let (cookie, token) = csrf_token(&app).await;

    let response = login(&app, &cookie, &token, Some(&basic("ann@example.com", "s3cret-pass"))).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["user"]["email"], "ann@example.com");
    assert_eq!(body["memberships"][0]["organization_id"], org.id.to_string());
}

#[sqlx::test]
async fn wrong_password_unknown_email_and_no_password_set_all_get_the_same_401(pool: PgPool) {
    ann_with_password(&pool).await;
    sqlx::query("UPDATE users SET email = 'oauth-only@example.com' WHERE email <> 'ann@example.com'").execute(&pool).await.ok();
    let app = app(pool.clone());
    let (cookie, token) = csrf_token(&app).await;
    let expected = serde_json::json!({"error": "invalid credentials"});

    for auth in [Some(basic("ann@example.com", "wrong")), Some(basic("nobody@example.com", "x")), None] {
        let response = login(&app, &cookie, &token, auth.as_deref()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body, expected);
    }
}

#[sqlx::test]
async fn login_never_creates_an_account(pool: PgPool) {
    let app = app(pool.clone());
    let (cookie, token) = csrf_token(&app).await;

    login(&app, &cookie, &token, Some(&basic("nobody@example.com", "x"))).await;

    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!(users, 0);
}

#[sqlx::test]
async fn the_route_is_csrf_protected(pool: PgPool) {
    ann_with_password(&pool).await;
    let app = app(pool);
    let (cookie, _) = csrf_token(&app).await;

    let response = login(&app, &cookie, "", Some(&basic("ann@example.com", "s3cret-pass"))).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test]
async fn a_successful_login_starts_a_working_session(pool: PgPool) {
    ann_with_password(&pool).await;
    let app = app(pool);
    let (cookie, token) = csrf_token(&app).await;

    let response = login(&app, &cookie, &token, Some(&basic("ann@example.com", "s3cret-pass"))).await;
    let logged_in = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();

    let me = app.clone().oneshot(Request::get("/auth/me").header(header::COOKIE, logged_in).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
}
