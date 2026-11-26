//! Authentication and PBAC permission primitives.

pub mod access_token;
pub mod audit;
pub mod db;
pub mod entity;
pub mod authorizer;
pub mod grant;
pub mod oauth;
pub mod user;

pub use access_token::AccessToken;
pub use authorizer::{request_action, request_grant, request_grants};
pub use authorizer::authorizer;
pub use grant::Grant;
pub use oauth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType, ProviderClient,
    CsrfToken, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
pub use oauth::oauth_http_client;
pub use user::{UpsertUserInput, User};

pub use user::{PasswordError, Registered, UserRepo};

pub mod http;

pub mod token;
pub mod client_credentials;

pub mod openapi;
pub mod rest;
pub mod crud;
pub mod filter;

pub use token::{csrf_unless_bearer, jwt_session, JwtSession, JWT_COOKIE};

pub use http::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};
