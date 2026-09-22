//! Standalone test server for Megh Gateway with live auth and test UI.

use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;
use tower_sessions::cookie::SameSite;
use tower_sessions::SessionManagerLayer;
use tower_sessions_sqlx_store::PostgresStore;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();
    dotenvy::dotenv().ok();

    let db_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://kyrios:kyrios@localhost:5432/kyrios?sslmode=disable".to_string());
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8080);
    let app_origin = std::env::var("APP_ORIGIN")
        .unwrap_or_else(|_| format!("http://localhost:{port}"));

    tracing::info!("Connecting to database: {db_url}");
    let pool = sqlx::PgPool::connect(&db_url).await?;

    // Run migrations
    sqlx::migrate!("./migrations").run(&pool).await?;
    tracing::info!("Database migrations applied");

    let google_client_id = std::env::var("GOOGLE_CLIENT_ID").unwrap_or_default();
    let google_client_secret = std::env::var("GOOGLE_CLIENT_SECRET").unwrap_or_default();

    let sessions = PostgresStore::new(pool.clone());
    sessions.migrate().await?;

    let mut megh_state = megh::MeghAuthState::new(pool)
        .with_app_origin(&app_origin)
        .with_redirect_after_login("/");

    if !google_client_id.is_empty() {
        tracing::info!("Registering Google OAuth provider with client_id: {google_client_id}");
        megh_state = megh_state.add_provider(
            megh::OAuthProviderConfig::google(google_client_id, Some(google_client_secret)),
        );
    } else {
        tracing::warn!("GOOGLE_CLIENT_ID not set; Google login will not be available");
    }

    let auth_router = megh::auth_router(megh_state).layer(SessionManagerLayer::new(sessions).with_same_site(SameSite::Lax));
    let serve_ui = ServeDir::new("ui/sdk");

    let app = axum::Router::new()
        .merge(auth_router)
        .fallback_service(serve_ui)
        .layer(CorsLayer::very_permissive());

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("Megh test server listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
