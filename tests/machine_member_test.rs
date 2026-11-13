//! Specification: `UserRepo::machine(tenant_id, name, grants)` is the tenant's machine member named `name`, created
//! with `grants` when the tenant has none of that name (issue #240). An app signs its own in-process calls as it.

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::get;
use axum::Router;
use megh::auth::{jwt_session, JwtSession, UserRepo};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"machine-member-test-secret-32byte";

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

fn grants(tenant: Uuid) -> Vec<String> {
    vec![format!("tenants:read:{tenant}"), "invoices".to_string()]
}

async fn machines(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM tenant_members WHERE role = 'machine'").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn the_first_call_creates_the_machine_member_with_its_grants(pool: PgPool) {
    let acme = tenant(&pool, "Acme").await;

    let member = UserRepo::new(&pool).machine(acme, "workflows", grants(acme)).await.unwrap();

    assert_eq!((member.tenant_id, member.role.as_str(), member.grants), (acme, "machine", grants(acme)));
    let (provider, name): (String, String) = sqlx::query_as("SELECT provider, display_name FROM users WHERE id = $1").bind(member.user_id).fetch_one(&pool).await.unwrap();
    assert_eq!((provider.as_str(), name.as_str()), ("megh", "workflows"));
}

#[sqlx::test]
async fn a_later_call_answers_the_same_member_and_creates_none(pool: PgPool) {
    let acme = tenant(&pool, "Acme").await;
    let users = UserRepo::new(&pool);

    let first = users.machine(acme, "workflows", grants(acme)).await.unwrap();
    let second = users.machine(acme, "workflows", grants(acme)).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(machines(&pool).await, 1);
}

#[sqlx::test]
async fn another_name_or_another_tenant_is_another_machine(pool: PgPool) {
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let users = UserRepo::new(&pool);

    let workflows = users.machine(acme, "workflows", grants(acme)).await.unwrap();
    let sync = users.machine(acme, "sync", grants(acme)).await.unwrap();
    let other = users.machine(globex, "workflows", grants(globex)).await.unwrap();

    assert_ne!(workflows.id, sync.id);
    assert_ne!(workflows.id, other.id);
    assert_eq!(other.tenant_id, globex);
    assert_eq!(machines(&pool).await, 3);
}

#[sqlx::test]
async fn a_token_issued_for_the_machine_enters_its_tenant_and_no_other(pool: PgPool) {
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let member = UserRepo::new(&pool).machine(acme, "workflows", grants(acme)).await.unwrap();
    let jwt = JwtSession::new(SECRET, Duration::from_secs(300), pool.clone());
    let token = jwt.issue(&member, &[]).unwrap().jwt;
    let app = Router::new()
        .route("/tenants/{tenant_id}/invoices", get(|| async { "ok" }))
        .layer(from_fn(megh::authorizer))
        .layer(from_fn_with_state(jwt, jwt_session))
        .layer(SessionManagerLayer::new(MemoryStore::default()));
    let status = |tenant: Uuid| {
        let request = Request::get(format!("/tenants/{tenant}/invoices")).header(header::AUTHORIZATION, format!("Bearer {token}"));
        let app = app.clone();
        async move { app.oneshot(request.body(Body::empty()).unwrap()).await.unwrap().status() }
    };

    assert_eq!(status(acme).await, StatusCode::OK);
    assert_eq!(status(globex).await, StatusCode::FORBIDDEN);
}
