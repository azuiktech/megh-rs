//! Megh: Micro Event-driven Gateway Hub in Rust.

pub mod account;
pub mod auth;
pub mod billing;
pub mod booking;
pub mod event;
pub mod messaging;
pub mod money;
pub mod tenant;
mod soft_delete;

pub use event::{
    Broker, Event, EventSource, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider,
    NoopHistoryProvider, RingBuffer, Topic,
};
pub use event::{events_router, events_router_with_csrf};

pub use auth::entity::{Array, JsonText, Table, Where};
pub use auth::db::{Db, On, Tx};
pub use megh_derive::{entity, sqlx_type};
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
pub use auth::oauth_http_client;
pub use auth::authorizer;
pub use auth::{PasswordError, UserRepo};

pub use auth::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};

pub use account::{ConnectedAccount, OAuth2Tokens};
pub use account::{ConnectedAccountRepo, EncryptError, Encryptor};
pub use account::{AccountAuth, AccountError, Accounts, Connections, Secret};
pub use account::connections_router;

pub use tenant::{Member, Tenant};
pub use tenant::{Change, Lifecycle, LifecycleError, TenantError, Tenants};

