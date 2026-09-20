//! Authentication and PBAC permission primitives.

pub mod authorizer;
pub mod grant;
pub mod oauth;
pub mod user;

pub use authorizer::{request_action, request_grant};
#[cfg(feature = "axum")]
pub use authorizer::authorizer;
pub use grant::Grant;
pub use oauth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType,
    CsrfToken, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
pub use user::{UpsertUserInput, User, UserProfile};

#[cfg(feature = "postgres")]
pub use user::UserRepo;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub mod http;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use http::{auth_router, AuthMeResponse, AuthUser, FromRef, MeghAuthState};
