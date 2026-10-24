//! Megh: Micro Event-driven Gateway Hub in Rust.

pub mod account;
#[cfg(feature = "postgres")]
pub mod audit;
pub mod auth;
pub mod billing;
pub mod booking;
pub mod entity;
pub mod event;
pub mod money;
pub mod org;
#[cfg(feature = "postgres")]
mod soft_delete;

pub use event::{
    Broker, Event, EventSource, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider,
    NoopHistoryProvider, RingBuffer, Topic,
};
#[cfg(feature = "axum")]
pub use event::{events_router, events_router_with_csrf};

pub use entity::{ColumnMeta, Entity, JsonText, Table, NOT_DELETED};
#[cfg(feature = "postgres")]
pub use entity::TableEntity;
pub use megh_derive::Table;
#[doc(hidden)]
pub use constcat as __constcat;

pub use auth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType, ProviderClient,
    CsrfToken, Grant, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, UpsertUserInput, User,
    UserProfile, request_action, request_grant, request_grants,
};
#[cfg(feature = "client")]
pub use auth::oauth_http_client;
#[cfg(feature = "axum")]
pub use auth::authorizer;
#[cfg(feature = "postgres")]
pub use auth::{PasswordError, UserRepo};

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use auth::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};

pub use account::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "postgres")]
pub use account::{ConnectedAccountRepo, EncryptError, Encryptor};
#[cfg(all(feature = "postgres", feature = "client"))]
pub use account::{AccountAuth, AccountError, Accounts};

pub use org::{Member, Org};
#[cfg(feature = "postgres")]
pub use org::{OrgError, Orgs};

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
    sqlx::raw_sql(include_str!("../migrations/0005_align_users.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0006_align_org.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0007_audit.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0008_soft_delete.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0009_booking_resources.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0010_booking_providers.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0011_booking_clients.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0012_booking_services.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0013_booking_schedules.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0014_booking_appointments.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0015_booking_availability.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0016_booking_reservations.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0018_billing_simplified.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0019_billing_counters.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0020_org_parent.sql"))
        .execute(pool)
        .await?;
    Ok(())
}
