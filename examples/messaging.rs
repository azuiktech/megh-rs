//! Messaging API server with OpenAPI docs at `/docs` and thread events at `/events`.
//!
//! `cargo run --example messaging`. Sign in with `POST /auth/login` (Basic credentials) for the `jwt_token` cookie, or
//! let a guest in with `POST /api/v1/tenants/{tenant_id}/guests`, which answers a bearer token for its one thread.

use std::sync::Arc;
use std::time::Duration;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::{Extension, Router};
use megh::auth::client_credentials::token_router;
use megh::auth::http::basic_login_router;
use megh::auth::rest::AppContext;
use megh::auth::{csrf_unless_bearer, jwt_session, JwtSession};
use megh::messaging::{guest_router, History, Messaging};
use megh::{auth_router, authorizer, events_router, Broker, MeghAuthState};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tower::util::MapRequestLayer;
use tower::Layer;
use tower_http::csrf::CsrfLayer;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;
use utoipa_axum::router::OpenApiRouter;
use utoipa_scalar::{Scalar, Servable};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let pool = PgPool::connect(&std::env::var("DATABASE_URL")?).await?;
    megh::migrate(&pool).await?;
    let sessions = PostgresStore::new(pool.clone());
    sessions.migrate().await?;
    let jwt = JwtSession::new(std::env::var("JWT_SECRET")?.as_bytes(), Duration::from_secs(15 * 60), pool.clone());
    let auth = MeghAuthState::new(pool.clone());
    let broker = Broker::new().with_authorizer(megh::messaging::authorizer()).with_history_provider(History::new(pool.clone()));
    let messaging = Messaging::new(pool.clone(), broker.clone());

    let (guarded, spec) = OpenApiRouter::with_openapi(megh::messaging::openapi()).nest("/api/v1", megh::messaging::router()).split_for_parts();
    let guarded = guarded
        .layer(from_fn(authorizer))
        .layer(from_fn_with_state(jwt.clone(), jwt_session))
        .route_layer(from_fn_with_state(CsrfLayer::new(), csrf_unless_bearer));
    let (public, _) = OpenApiRouter::new().nest("/api/v1", guest_router()).split_for_parts();
    let app = Router::new()
        .merge(guarded)
        .merge(public)
        .layer(Extension(messaging))
        .layer(Extension(jwt.clone()))
        .with_state(AppContext { pool })
        .nest("/events", events_router(Arc::new(broker)).layer(from_fn_with_state(jwt.clone(), jwt_session)))
        .merge(auth_router(auth.clone()))
        .merge(token_router(auth.clone(), jwt))
        .merge(basic_login_router(auth, "/auth/login"))
        .merge(Scalar::with_url("/docs", spec))
        .layer(SessionManagerLayer::new(sessions));
    let app = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app);

    let address = std::env::var("ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".into());
    println!("messaging API on http://{address}, docs at http://{address}/docs");
    axum::serve(TcpListener::bind(&address).await?, axum::ServiceExt::into_make_service(app)).await?;
    Ok(())
}
