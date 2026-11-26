use axum::body::Body;
#[macro_use]
mod common;

use axum::http::{header, Request, StatusCode};
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use megh::auth::http::{auth_router, basic_login_router, MeghAuthState};
use megh::{Tenant, UserRepo};
use serde_json::Value;
use megh::Db;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

fn app(pool: Db) -> axum::Router {
    let state = MeghAuthState::new(pool);
    auth_router(state.clone()).merge(basic_login_router(state, "/auth/login")).layer(SessionManagerLayer::new(MemoryStore::default()))
}

fn basic(email: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{email}:{password}")))
}

async fn login_from(app: &axum::Router, site: &str, auth: Option<&str>) -> axum::http::Response<Body> {
    let mut request = Request::post("/auth/login").header("sec-fetch-site", site);
    if let Some(auth) = auth {
        request = request.header(header::AUTHORIZATION, auth);
    }
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap()
}

async fn login(app: &axum::Router, auth: Option<&str>) -> axum::http::Response<Body> {
    login_from(app, "same-origin", auth).await
}

async fn ann_with_password(pool: &Db) -> Uuid {
    let repo = UserRepo::new(pool);
    let user = repo
        .upsert(&megh::UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None })
        .await
        .unwrap();
    repo.set_password(user.id, "s3cret-pass").await.unwrap();
    user.id
}

dual!(correct_credentials_log_in_and_return_memberships);
async fn correct_credentials_log_in_and_return_memberships(pool: Db) {
    let user_id = ann_with_password(&pool).await;
    let tenant: Tenant = Db::query_as("INSERT INTO tenants (name) VALUES ('Acme') RETURNING *").fetch_one(&pool).await.unwrap();
    Db::query("INSERT INTO tenant_members (tenant_id, user_id, grants) VALUES ($1, $2, '[\"invoices:read\"]')").bind(tenant.id).bind(user_id).execute(&pool).await.unwrap();
    let app = app(pool);

    let response = login(&app, Some(&basic("ann@example.com", "s3cret-pass"))).await;

    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert_eq!(body["user"]["email"], "ann@example.com");
    assert_eq!(body["memberships"][0]["tenant_id"], tenant.id.to_string());
}

dual!(wrong_password_unknown_email_and_no_password_set_all_get_the_same_401);
async fn wrong_password_unknown_email_and_no_password_set_all_get_the_same_401(pool: Db) {
    ann_with_password(&pool).await;
    Db::query("UPDATE users SET email = 'oauth-only@example.com' WHERE email <> 'ann@example.com'").execute(&pool).await.ok();
    let app = app(pool.clone());
    let expected = serde_json::json!({"error": "invalid credentials"});

    for auth in [Some(basic("ann@example.com", "wrong")), Some(basic("nobody@example.com", "x")), None] {
        let response = login(&app, auth.as_deref()).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
        assert_eq!(body, expected);
    }
}

dual!(login_never_creates_an_account);
async fn login_never_creates_an_account(pool: Db) {
    let app = app(pool.clone());

    login(&app, Some(&basic("nobody@example.com", "x"))).await;

    let users: i64 = Db::query_scalar("SELECT count(*) FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!(users, 0);
}

dual!(the_route_is_csrf_protected);
async fn the_route_is_csrf_protected(pool: Db) {
    ann_with_password(&pool).await;
    let app = app(pool);

    let response = login_from(&app, "cross-site", Some(&basic("ann@example.com", "s3cret-pass"))).await;

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

dual!(a_successful_login_starts_a_working_session);
async fn a_successful_login_starts_a_working_session(pool: Db) {
    ann_with_password(&pool).await;
    let app = app(pool);

    let response = login(&app, Some(&basic("ann@example.com", "s3cret-pass"))).await;
    let logged_in = response.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();

    let me = app.clone().oneshot(Request::get("/auth/me").header(header::COOKIE, logged_in).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(me.status(), StatusCode::OK);
}
