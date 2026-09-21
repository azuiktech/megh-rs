//! Connected accounts of users at identity providers, kept usable: stored tokens are refreshed when they expire.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use oauth2::basic::BasicErrorResponseType;
use oauth2::http::Extensions;
use oauth2::{RefreshToken, RequestTokenError};
use reqwest::header::{HeaderValue, AUTHORIZATION};
use sqlx::PgPool;

use super::model::{ConnectedAccount, OAuth2Tokens};
use super::repo::ConnectedAccountRepo;
use crate::auth::{oauth_http_client, OAuthProviderConfig, User};

/// Failure to obtain a usable access token for a connected account.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AccountError {
    #[error("connected account not found")]
    NotFound,
    #[error("connected account is disconnected")]
    Disconnected,
    #[error("no refresh token is stored for the account")]
    NoRefreshToken,
    #[error("provider {0} is not configured")]
    NotConfigured(String),
    #[error("invalid_grant: {0}")]
    InvalidGrant(String),
    #[error("token request failed: {0}")]
    Provider(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// Connected accounts and their tokens. Holds the injected pool and the configured providers.
#[derive(Clone)]
pub struct Accounts {
    pool: PgPool,
    providers: Arc<HashMap<String, OAuthProviderConfig>>,
    http: reqwest::Client,
    expiry_margin: Duration,
}

impl Accounts {
    pub fn new(pool: PgPool, providers: Arc<HashMap<String, OAuthProviderConfig>>) -> Self {
        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .expect("static HTTP client configuration");
        Self { pool, providers, http, expiry_margin: Duration::from_secs(60) }
    }

    /// The client used for token requests to the provider. It must not follow redirects.
    pub fn with_http_client(mut self, http: reqwest::Client) -> Self {
        self.http = http;
        self
    }

    /// Tokens expiring within this margin are refreshed (default 60 s).
    pub fn with_expiry_margin(mut self, margin: Duration) -> Self {
        self.expiry_margin = margin;
        self
    }

    /// A `reqwest-middleware` layer that authenticates requests as the account, refreshing its token when needed.
    pub fn auth(&self, provider: &str, account_id: &str) -> AccountAuth {
        AccountAuth { accounts: self.clone(), provider: provider.to_string(), account_id: account_id.to_string() }
    }

    /// Like `auth`, for the user's account at the provider (users and accounts are linked by email).
    pub async fn auth_for(&self, user: &User, provider: &str) -> Result<AccountAuth, AccountError> {
        let account = ConnectedAccountRepo::new(&self.pool)
            .find_by_email_or_account(&user.email, provider)
            .await?
            .ok_or(AccountError::NotFound)?;
        Ok(self.auth(provider, &account.account_id))
    }

    /// The account's access token, refreshed first when expired. The row lock makes concurrent callers refresh once.
    async fn access_token(&self, provider: &str, account_id: &str) -> Result<String, AccountError> {
        let mut tx = self.pool.begin().await?;
        let account = sqlx::query_as::<_, ConnectedAccount>("SELECT * FROM connected_accounts WHERE account_id = $1 AND provider = $2 FOR UPDATE")
            .bind(account_id)
            .bind(provider)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(AccountError::NotFound)?;
        if !account.is_connected() {
            return Err(AccountError::Disconnected);
        }
        if !account.is_expired(self.expiry_margin.as_secs() as i64) {
            return Ok(account.access_token);
        }
        let refresh_token = account.refresh_token.ok_or(AccountError::NoRefreshToken)?;
        match self.request_refresh(provider, refresh_token).await {
            Ok(tokens) => {
                sqlx::query("UPDATE connected_accounts SET access_token = $3, refresh_token = COALESCE($4, refresh_token), expiry = $5, updated_at = NOW() WHERE account_id = $1 AND provider = $2")
                    .bind(account_id)
                    .bind(provider)
                    .bind(&tokens.access_token)
                    .bind(tokens.refresh_token)
                    .bind(tokens.token_expires_at)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Ok(tokens.access_token)
            }
            Err(error @ AccountError::InvalidGrant(_)) => {
                sqlx::query("UPDATE connected_accounts SET disconnected_at = NOW(), updated_at = NOW() WHERE account_id = $1 AND provider = $2")
                    .bind(account_id)
                    .bind(provider)
                    .execute(&mut *tx)
                    .await?;
                tx.commit().await?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    async fn request_refresh(&self, provider: &str, refresh_token: String) -> Result<OAuth2Tokens, AccountError> {
        let config = self.providers.get(provider).ok_or_else(|| AccountError::NotConfigured(provider.to_string()))?;
        let client = config.build_client(None).map_err(|e| AccountError::Provider(e.to_string()))?;
        client
            .exchange_refresh_token(&RefreshToken::new(refresh_token))
            .request_async(&oauth_http_client(self.http.clone()))
            .await
            .map(|response| OAuth2Tokens::from(&response))
            .map_err(|error| match error {
                RequestTokenError::ServerResponse(response) if matches!(response.error(), BasicErrorResponseType::InvalidGrant) => {
                    AccountError::InvalidGrant(response.error_description().cloned().unwrap_or_default())
                }
                other => AccountError::Provider(other.to_string()),
            })
    }
}

/// Authenticates requests as a connected account; add it to your own `reqwest` client with `reqwest_middleware::ClientBuilder::with`.
/// A failure to get a token surfaces as `reqwest_middleware::Error::Middleware` holding an `AccountError`.
pub struct AccountAuth {
    accounts: Accounts,
    provider: String,
    account_id: String,
}

#[async_trait::async_trait]
impl reqwest_middleware::Middleware for AccountAuth {
    async fn handle(&self, mut request: reqwest::Request, extensions: &mut Extensions, next: reqwest_middleware::Next<'_>) -> reqwest_middleware::Result<reqwest::Response> {
        let token = self.accounts.access_token(&self.provider, &self.account_id).await.map_err(|e| reqwest_middleware::Error::Middleware(e.into()))?;
        let mut value = HeaderValue::from_str(&format!("Bearer {token}")).map_err(|e| reqwest_middleware::Error::Middleware(e.into()))?;
        value.set_sensitive(true);
        request.headers_mut().insert(AUTHORIZATION, value);
        next.run(request, extensions).await
    }
}
