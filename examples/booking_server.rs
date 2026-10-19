//! The booking API with its OpenAPI reference and a browser UI: open `/docs`, sign in and try the calls.
//!
//! Needs `DATABASE_URL` and `JWT_SECRET`. To have a user to sign in as, set `BOOKING_EMAIL` and `BOOKING_PASSWORD`:
//! the server then makes that user (with a password and every grant) a member of an org named `Booking`, whose id
//! it logs.

use std::net::SocketAddr;
use std::time::Duration;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::Router;
use megh::auth::http::basic_login_router;
use megh::auth::{jwt_session, JwtSession};
use megh::{authorizer, UpsertUserInput, UserRepo};
use sqlx::PgPool;
use tower_sessions::cookie::SameSite;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;
use utoipa_swagger_ui::SwaggerUi;

/// A member with every grant in an org named `Booking`, for the user of the email and password.
async fn seed(pool: &PgPool, email: &str, password: &str) -> Result<(), Box<dyn std::error::Error>> {
    let users = UserRepo::new(pool);
    let user = users.upsert(&UpsertUserInput { provider: "password".into(), account_id: email.into(), email: email.into(), display_name: None, photo_url: None }).await?;
    users.set_password(user.id, password).await?;
    let org: uuid::Uuid = match sqlx::query_scalar("SELECT id FROM organizations WHERE name = 'Booking'").fetch_optional(pool).await? {
        Some(org) => org,
        None => sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Booking') RETURNING id").fetch_one(pool).await?,
    };
    sqlx::query("INSERT INTO organization_members (organization_id, user_id, role, grants) VALUES ($1, $2, 'owner', '[\"*\"]') ON CONFLICT (organization_id, user_id) DO UPDATE SET grants = '[\"*\"]'")
        .bind(org)
        .bind(user.id)
        .execute(pool)
        .await?;
    tracing::info!("sign in as {email}; the org is {org}");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();
    let port: u16 = std::env::var("PORT").ok().and_then(|port| port.parse().ok()).unwrap_or(8080);
    let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    megh::migrate(&pool).await?;
    if let (Ok(email), Ok(password)) = (std::env::var("BOOKING_EMAIL"), std::env::var("BOOKING_PASSWORD")) {
        seed(&pool, &email, &password).await?;
    }

    let sessions = PostgresStore::new(pool.clone());
    sessions.migrate().await?;
    let jwt = JwtSession::new(std::env::var("JWT_SECRET")?.as_bytes(), Duration::from_secs(3600), pool.clone());
    let state = megh::MeghAuthState::new(pool.clone()).with_app_origin(&format!("http://localhost:{port}"));

    let booking = megh::booking::booking_router(pool).layer(from_fn(authorizer)).layer(from_fn_with_state(jwt, jwt_session));
    let app = Router::new()
        .merge(megh::auth_router(state.clone()))
        .merge(basic_login_router(state, "/auth/login"))
        .merge(booking)
        .layer(SessionManagerLayer::new(sessions).with_same_site(SameSite::Lax))
        .merge(SwaggerUi::new("/docs").url("/openapi.json", megh::booking::openapi()));

    let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], port))).await?;
    tracing::info!("http://localhost:{port}/docs");
    axum::serve(listener, app).await?;
    Ok(())
}
