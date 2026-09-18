//! OAuth 2.0 integration leveraging the standard `oauth2` crate.

use serde::{Deserialize, Serialize};
use thiserror::Error;

// Re-export standard oauth2 types so consumers don't need to depend on oauth2 directly.
pub use oauth2::basic::{BasicClient, BasicTokenResponse, BasicTokenType};
pub use oauth2::reqwest::async_http_client;
pub use oauth2::url::{self, Url};
pub use oauth2::{
    AccessToken, AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, RedirectUrl, RefreshToken, Scope, TokenResponse, TokenUrl,
};

/// OAuth specific error variants.
#[derive(Debug, Error)]
pub enum OAuthError {
    #[error("Failed to parse URL: {0}")]
    UrlParse(#[from] oauth2::url::ParseError),

    #[error("HTTP transport error: {0}")]
    #[cfg(feature = "client")]
    Http(#[from] reqwest::Error),

    #[error("OAuth token exchange error: {0}")]
    TokenExchange(String),

    #[error("Missing token or user information in response: {0}")]
    InvalidResponse(String),
}

/// Supported OAuth authorization flow modes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum OAuthFlowMode {
    /// Standard Web server-side redirect flow with a hosted backend callback URL.
    WebRedirect { redirect_uri: String },
    /// Desktop or CLI loopback redirect flow (e.g. `http://127.0.0.1:8080/callback`).
    DesktopLoopback { redirect_uri: String },
    /// Native or desktop app custom URI scheme (e.g. `kyrios://oauth/callback`).
    CustomScheme { redirect_uri: String },
}

impl OAuthFlowMode {
    pub fn redirect_uri(&self) -> &str {
        match self {
            OAuthFlowMode::WebRedirect { redirect_uri } => redirect_uri,
            OAuthFlowMode::DesktopLoopback { redirect_uri } => redirect_uri,
            OAuthFlowMode::CustomScheme { redirect_uri } => redirect_uri,
        }
    }

    pub fn to_redirect_url(&self) -> Result<RedirectUrl, OAuthError> {
        RedirectUrl::new(self.redirect_uri().to_string()).map_err(OAuthError::UrlParse)
    }
}

/// OAuth 2.0 provider configuration (e.g. Google, GitHub, Apple, HubSpot).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthProviderConfig {
    pub provider_id: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub auth_url: String,
    pub token_url: String,
    pub userinfo_url: Option<String>,
    pub default_scopes: Vec<String>,
    pub redirect_url: Option<String>,
}

impl OAuthProviderConfig {
    /// Factory for standard Google OAuth 2.0 / OIDC provider.
    pub fn google(client_id: impl Into<String>, client_secret: Option<String>) -> Self {
        Self {
            provider_id: "google".to_string(),
            client_id: client_id.into(),
            client_secret,
            auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            token_url: "https://oauth2.googleapis.com/token".to_string(),
            userinfo_url: Some("https://openidconnect.googleapis.com/v1/userinfo".to_string()),
            default_scopes: vec![
                "openid".to_string(),
                "https://www.googleapis.com/auth/userinfo.email".to_string(),
                "https://www.googleapis.com/auth/userinfo.profile".to_string(),
            ],
            redirect_url: None,
        }
    }

    pub fn with_redirect_url(mut self, redirect_url: impl Into<String>) -> Self {
        self.redirect_url = Some(redirect_url.into());
        self
    }

    /// Builds a standard `oauth2::basic::BasicClient` from this configuration.
    pub fn build_client(&self, redirect_url: Option<RedirectUrl>) -> Result<BasicClient, OAuthError> {
        let auth_url = AuthUrl::new(self.auth_url.clone())?;
        let token_url = TokenUrl::new(self.token_url.clone())?;
        let client_id = ClientId::new(self.client_id.clone());
        let client_secret = self.client_secret.clone().map(ClientSecret::new);

        let mut client = BasicClient::new(client_id, client_secret, auth_url, Some(token_url));
        if let Some(redirect) = redirect_url {
            client = client.set_redirect_uri(redirect);
        }
        Ok(client)
    }
}

/// Options when building an OAuth authorization URL using the library.
#[derive(Debug, Clone, Default)]
pub struct AuthUrlOptions<'a> {
    /// Requested scopes (e.g. `openid`, `email`, or calendar/gmail scopes).
    pub scopes: &'a [&'a str],
    /// Optional PKCE challenge (recommended for desktop/native clients).
    pub pkce: Option<&'a PkceCodeChallenge>,
    /// Request offline access (refresh token) when true (`access_type=offline`).
    pub offline_access: bool,
    /// Enable incremental authorization where supported (e.g. Google `include_granted_scopes=true`).
    pub incremental: bool,
    /// Prompt behavior (e.g. `consent`, `select_account`).
    pub prompt: Option<&'a str>,
}

/// Builds an authorization URL using `oauth2::basic::BasicClient`.
pub fn build_authorization_url(
    client: &BasicClient,
    csrf_token: CsrfToken,
    opts: AuthUrlOptions,
) -> Url {
    let mut builder = client.authorize_url(|| csrf_token);

    for scope in opts.scopes {
        builder = builder.add_scope(Scope::new(scope.to_string()));
    }

    if let Some(pkce) = opts.pkce {
        builder = builder.set_pkce_challenge(pkce.clone());
    }

    if opts.offline_access {
        builder = builder.add_extra_param("access_type", "offline");
    }

    if opts.incremental {
        builder = builder.add_extra_param("include_granted_scopes", "true");
    }

    if let Some(prompt) = opts.prompt {
        builder = builder.add_extra_param("prompt", prompt);
    }

    let (url, _) = builder.url();
    url
}

/// Normalized user profile information retrieved from identity providers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuthUserInfo {
    pub subject: String,
    pub email: String,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// Fetches user profile information from the provider's userinfo endpoint.
#[cfg(feature = "client")]
pub async fn fetch_user_info(
    http_client: &reqwest::Client,
    userinfo_url: &str,
    access_token: &str,
) -> Result<OAuthUserInfo, OAuthError> {
    let resp = http_client
        .get(userinfo_url)
        .bearer_auth(access_token)
        .send()
        .await?;

    if !resp.status().is_success() {
        let error_body = resp.text().await.unwrap_or_default();
        return Err(OAuthError::InvalidResponse(error_body));
    }

    #[derive(Deserialize)]
    struct StandardUserInfo {
        sub: Option<String>,
        id: Option<String>,
        email: Option<String>,
        email_verified: Option<bool>,
        name: Option<String>,
        picture: Option<String>,
        avatar_url: Option<String>,
    }

    let raw = resp.json::<StandardUserInfo>().await?;
    let subject = raw
        .sub
        .or(raw.id)
        .ok_or_else(|| OAuthError::InvalidResponse("missing subject/id in userinfo".to_string()))?;
    let email = raw
        .email
        .ok_or_else(|| OAuthError::InvalidResponse("missing email in userinfo".to_string()))?;

    Ok(OAuthUserInfo {
        subject,
        email,
        email_verified: raw.email_verified,
        name: raw.name,
        picture: raw.picture.or(raw.avatar_url),
    })
}
