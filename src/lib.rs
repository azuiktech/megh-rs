//! Megh: Micro Event-driven Gateway Hub in Rust.

pub mod auth;
pub mod connection;
pub mod entity;
pub mod org;
pub mod session;

pub use entity::Entity;

pub use auth::{
    build_authorization_url, AuthUrlOptions, BasicClient, BasicTokenResponse, BasicTokenType,
    CsrfToken, Grant, OAuthError, OAuthFlowMode, OAuthProviderConfig, OAuthUserInfo,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse, UpsertUserInput, User,
    UserProfile,
};
#[cfg(feature = "postgres")]
pub use auth::UserRepo;

pub use connection::{
    Connection, ConnectionData, ConnectionExt, ConnectionView, FullConnection, OAuth2Tokens,
    UpsertConnectionInput,
};
#[cfg(feature = "postgres")]
pub use connection::ConnectionRepo;

pub use org::{Org, OrgMember};

pub use session::{
    generate_session_token, hash_session_token, CreatedSession, FullSession, Session, SessionData,
    SessionExt, SessionSecrets, SessionView,
};
#[cfg(feature = "postgres")]
pub use session::SessionRepo;
