//! Authentication and PBAC permission primitives.

pub mod authorizer;
pub mod grant;
pub mod oauth;
pub mod user;

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
pub use user::{UpsertUserInput, User, UserProfile};

#[cfg(feature = "postgres")]
pub use user::{PasswordError, Registered, UserRepo};

#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod http;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod token;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod client_credentials;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod crud;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod filter;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod openapi;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod rest;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use token::{csrf_unless_bearer, jwt_session, AccessToken, JwtSession, JWT_COOKIE};

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use http::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};
