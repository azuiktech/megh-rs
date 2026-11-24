//! Megh: Micro Event-driven Gateway Hub in Rust.

pub mod account;
pub mod auth;
pub mod billing;
pub mod booking;
pub mod event;
pub mod messaging;
pub mod money;
pub mod tenant;
#[cfg(feature = "postgres")]
mod soft_delete;

pub use event::{
    Broker, Event, EventSource, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider,
    NoopHistoryProvider, RingBuffer, Topic,
};
#[cfg(feature = "axum")]
pub use event::{events_router, events_router_with_csrf};

pub use auth::entity::{ColumnMeta, JsonText, Table};
pub use megh_derive::{entity, sqlx_type, Column};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
#[doc(hidden)]
pub use sqlx as __sqlx;

#[cfg(feature = "postgres")]
#[doc(hidden)]
#[macro_export]
macro_rules! __if_postgres {
    ($($item:item)*) => { $($item)* };
}
#[cfg(not(feature = "postgres"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __if_postgres {
    ($($item:item)*) => {};
}
#[cfg(feature = "sqlite")]
#[doc(hidden)]
#[macro_export]
macro_rules! __if_sqlite {
    ($($item:item)*) => { $($item)* };
}
#[cfg(not(feature = "sqlite"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __if_sqlite {
    ($($item:item)*) => {};
}
#[cfg(feature = "axum")]
#[doc(hidden)]
pub use axum as __axum;
pub use sea_query;
#[doc(hidden)]
pub use constcat as __constcat;

pub use auth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType, ProviderClient,
    CsrfToken, Grant, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, UpsertUserInput, User,
    request_action, request_grant, request_grants,
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
pub use account::{AccountAuth, AccountError, Accounts, Connections, Secret};
#[cfg(all(feature = "axum", feature = "postgres", feature = "client"))]
pub use account::connections_router;

pub use tenant::{Member, Tenant};
#[cfg(feature = "postgres")]
pub use tenant::{Change, Lifecycle, LifecycleError, TenantError, Tenants};

#[cfg(feature = "postgres")]
/// Executes all embedded database migrations for Megh foundation tables.
pub async fn migrate(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(include_str!("../migrations/0001_auth.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0002_audit.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0003_tenant.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0004_booking.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0005_billing.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0006_products.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0007_connections.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0008_connection_spec.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0009_messages.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0010_attachments.sql"))
        .execute(pool)
        .await?;
    sqlx::raw_sql(include_str!("../migrations/0011_client_moved.sql"))
        .execute(pool)
        .await?;
    Ok(())
}
