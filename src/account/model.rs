//! Domain model for third-party ConnectedAccount matching Go megh connected_accounts table.

use chrono::{DateTime, Duration, Utc};
use oauth2::basic::BasicTokenResponse;
use oauth2::TokenResponse;
use serde::{Deserialize, Serialize};

use crate::Table;

/// Third-party OAuth connected account stored in `connected_accounts`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "connected_accounts", keys = ["account_id", "provider"])]
pub struct ConnectedAccount {
    pub account_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: Option<String>,
    pub expiry: Option<DateTime<Utc>>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub disconnected_at: Option<DateTime<Utc>>,
}

impl ConnectedAccount {
    /// Returns true if the account is currently active (not disconnected).
    pub fn is_connected(&self) -> bool {
        self.disconnected_at.is_none()
    }

    /// Checks if the access token has expired or will expire within `margin_secs`.
    pub fn is_expired(&self, margin_secs: i64) -> bool {
        self.expiry
            .map(|exp| Utc::now() + Duration::seconds(margin_secs) >= exp)
            .unwrap_or(false)
    }
}

/// Helper container for token extraction from OAuth2 responses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OAuth2Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
}

impl OAuth2Tokens {
    /// Checks if the access token has expired or will expire within `margin_secs`.
    pub fn is_expired(&self, margin_secs: i64) -> bool {
        self.token_expires_at
            .map(|exp| Utc::now() + Duration::seconds(margin_secs) >= exp)
            .unwrap_or(false)
    }
}

impl From<&BasicTokenResponse> for OAuth2Tokens {
    fn from(resp: &BasicTokenResponse) -> Self {
        let token_expires_at = resp.expires_in().map(|dur| {
            Utc::now() + Duration::from_std(dur).unwrap_or_default()
        });

        Self {
            access_token: resp.access_token().secret().to_string(),
            refresh_token: resp.refresh_token().map(|r| r.secret().to_string()),
            token_expires_at,
        }
    }
}
