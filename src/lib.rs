//! Megh: Micro Event-driven Gateway Hub in Rust.

pub mod account;
pub mod auth;
pub mod entity;
pub mod event;
pub mod org;
pub mod session;

pub use event::{
    Broker, Event, EventSource, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider,
    NoopHistoryProvider, RingBuffer, Topic,
};
#[cfg(feature = "axum")]
pub use event::events_router;

pub use entity::{ColumnMeta, Entity, Table};
#[cfg(feature = "postgres")]
pub use entity::TableEntity;
pub use megh_derive::Table;

pub use auth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType,
    CsrfToken, Grant, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, UpsertUserInput, User,
    UserProfile, request_action, request_grant,
};
#[cfg(feature = "axum")]
pub use auth::authorizer;
#[cfg(feature = "postgres")]
pub use auth::UserRepo;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use auth::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};

pub use account::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "postgres")]
pub use account::ConnectedAccountRepo;

pub use org::{Org, OrgMember};

pub use session::{
    generate_session_token, hash_session_token, CreatedSession, FullSession, Session, SessionData,
    SessionExt, SessionSecrets, SessionView,
};
#[cfg(feature = "postgres")]
pub use session::SessionRepo;

#[cfg(feature = "postgres")]
/// Executes all embedded database migrations for Megh foundation tables.
pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(include_str!("../migrations/0001_create_org.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0002_create_users.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0003_create_connected_accounts.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0004_create_sessions.sql"))
        .execute(pool)
        .await?;
    Ok(())
}
