//! Authentication and PBAC permission primitives.

pub mod grant;
pub mod oauth;
pub mod user;

pub use grant::Grant;
pub use oauth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType,
    CsrfToken, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
pub use user::{UpsertUserInput, User};

#[cfg(feature = "postgres")]
pub use user::UserRepo;
