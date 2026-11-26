//! Authentication and PBAC permission primitives.

#[cfg(feature = "axum")]
pub mod access_token;
pub mod audit;
#[cfg(feature = "sql")]
pub mod db;
pub mod entity;
pub mod authorizer;
pub mod grant;
pub mod oauth;
pub mod user;

#[cfg(feature = "axum")]
pub use access_token::AccessToken;
pub use authorizer::{request_action, request_grant, request_grants};
#[cfg(feature = "axum")]
pub use authorizer::authorizer;
pub use grant::Grant;
pub use oauth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType, ProviderClient,
    CsrfToken, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
#[cfg(feature = "client")]
pub use oauth::oauth_http_client;
pub use user::{UpsertUserInput, User};

#[cfg(feature = "sql")]
pub use user::{PasswordError, Registered, UserRepo};

#[cfg(all(feature = "axum", feature = "sql"))]
pub mod http;

#[cfg(all(feature = "axum", feature = "sql"))]
pub mod token;
#[cfg(all(feature = "axum", feature = "sql"))]
pub mod client_credentials;

#[cfg(all(feature = "axum", feature = "sql"))]
pub mod openapi;
#[cfg(all(feature = "axum", feature = "sql"))]
pub mod rest;
#[cfg(all(feature = "axum", feature = "sql"))]
pub mod crud;
#[cfg(all(feature = "axum", feature = "sql"))]
pub mod filter;

#[cfg(all(feature = "axum", feature = "sql"))]
pub use token::{csrf_unless_bearer, jwt_session, JwtSession, JWT_COOKIE};

#[cfg(all(feature = "axum", feature = "sql"))]
pub use http::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};
