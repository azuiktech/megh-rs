//! Booking and billing API server with OpenAPI docs at `/docs`.
//!
//! `cargo run --example booking`. Sign in with `POST /auth/login` (Basic credentials); the session then carries the `jwt_token` cookie every booking and billing route needs.

use std::time::Duration;

use axum::middleware::{from_fn, from_fn_with_state};
use axum::{Extension, Router};
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::auth::rest::AppContext;
use megh::auth::client_credentials::token_router;
use megh::auth::http::basic_login_router;
use megh::auth::{csrf_unless_bearer, jwt_session, JwtSession};
use megh::{auth_router, authorizer, MeghAuthState};
use sqlx::PgPool;
use tokio::net::TcpListener;
use tower::Layer;
use tower_http::csrf::CsrfLayer;
use tower::util::MapRequestLayer;
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

    let routes = megh::booking::router().merge(megh::billing::router());
    let (api, spec) = OpenApiRouter::with_openapi(megh::booking::openapi()).nest("/api/v1", routes).split_for_parts();
    let api = api
        .layer(from_fn(authorizer))
        .layer(Extension(Formats::new(Numbering::new("INV", "-", Middle::Year, 4)?, Numbering::new("PAY", "-", Middle::None, 4)?)))
        .layer(from_fn_with_state(jwt.clone(), jwt_session))
        .route_layer(from_fn_with_state(CsrfLayer::new(), csrf_unless_bearer))
        .with_state(AppContext { pool });
    let app = Router::new()
        .merge(api)
        .merge(auth_router(auth.clone()))
        .merge(token_router(auth.clone(), jwt))
        .merge(basic_login_router(auth, "/auth/login"))
        .merge(Scalar::with_url("/docs", spec))
        .layer(SessionManagerLayer::new(sessions));
    let app = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app);

    let address = std::env::var("ADDRESS").unwrap_or_else(|_| "127.0.0.1:8080".into());
    println!("booking and billing API on http://{address}, docs at http://{address}/docs");
    axum::serve(TcpListener::bind(&address).await?, axum::ServiceExt::into_make_service(app)).await?;
    Ok(())
}
