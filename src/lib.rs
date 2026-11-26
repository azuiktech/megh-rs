//! Megh: Micro Event-driven Gateway Hub in Rust.

pub mod account;
pub mod auth;
pub mod billing;
pub mod booking;
pub mod event;
pub mod messaging;
pub mod money;
pub mod tenant;
#[cfg(feature = "sql")]
mod soft_delete;

pub use event::{
    Broker, Event, EventSource, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider,
    NoopHistoryProvider, RingBuffer, Topic,
};
#[cfg(feature = "axum")]
pub use event::{events_router, events_router_with_csrf};

pub use auth::entity::{Array, JsonText, Table, Where};
#[cfg(feature = "sql")]
pub use auth::db::{Db, On, Tx};
pub use megh_derive::{entity, sqlx_type};
#[cfg(feature = "sql")]
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
#[cfg(feature = "sql")]
pub use auth::{PasswordError, UserRepo};

#[cfg(all(feature = "axum", feature = "sql"))]
pub use auth::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};

pub use account::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "sql")]
pub use account::{ConnectedAccountRepo, EncryptError, Encryptor};
#[cfg(all(feature = "sql", feature = "client"))]
pub use account::{AccountAuth, AccountError, Accounts, Connections, Secret};
#[cfg(all(feature = "axum", feature = "sql", feature = "client"))]
pub use account::connections_router;

pub use tenant::{Member, Tenant};
#[cfg(feature = "sql")]
pub use tenant::{Change, Lifecycle, LifecycleError, TenantError, Tenants};

