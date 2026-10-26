//! Issue #213 (#6 of the code review, RFC 9068 §4): an access token is for one tenant, its `aud`. A route of another
//! tenant refuses it whatever its grants, and so does a route of another tenant for a member.

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::get;
use axum::{Extension, Router};
use chrono::Utc;
use megh::auth::{jwt_session, JwtSession};
use megh::{authorizer, Member};
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"tenant-audience-test-secret-32-b";

/// A member of `tenant` whose grants name every tenant.
fn member(tenant: Uuid) -> Member {
    Member {
        id: Uuid::now_v7(),
        tenant_id: tenant,
        user_id: Uuid::now_v7(),
        role: "member".into(),
        grants: vec!["tenants:read:*".into(), "invoices:*".into()],
        joined_at: Utc::now(),
        invited_by: None,
    }
}

fn routes() -> Router {
    Router::new().route("/tenants/{tenant_id}/invoices", get(|| async { "ok" })).layer(from_fn(authorizer))
}

async fn status(app: &Router, tenant: Uuid, bearer: Option<&str>) -> StatusCode {
    let request = Request::get(format!("/tenants/{tenant}/invoices"));
    let request = match bearer {
        Some(jwt) => request.header(header::AUTHORIZATION, format!("Bearer {jwt}")),
        None => request,
    };
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap().status()
}

#[sqlx::test]
async fn a_token_is_refused_on_another_tenants_route(pool: PgPool) {
    let (mine, other) = (Uuid::now_v7(), Uuid::now_v7());
    let jwt = JwtSession::new(SECRET, Duration::from_secs(300), pool);
    let issued = jwt.issue(&member(mine), &[]).unwrap();
    let app = routes().layer(from_fn_with_state(jwt, jwt_session)).layer(SessionManagerLayer::new(MemoryStore::default()));

    assert_eq!(status(&app, mine, Some(&issued.jwt)).await, StatusCode::OK);
    assert_eq!(status(&app, other, Some(&issued.jwt)).await, StatusCode::FORBIDDEN, "tenants:read:* does not reach another tenant");
}

#[tokio::test]
async fn a_member_is_refused_on_another_tenants_route() {
    let (mine, other) = (Uuid::now_v7(), Uuid::now_v7());
    let app = routes().layer(Extension(member(mine)));

    assert_eq!(status(&app, mine, None).await, StatusCode::OK);
    assert_eq!(status(&app, other, None).await, StatusCode::FORBIDDEN);
}
