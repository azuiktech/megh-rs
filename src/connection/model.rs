//! Connection models using the generic Entity<ID, T> pattern for third-party OAuth integrations.

use std::ops::{Deref, DerefMut};
use chrono::{DateTime, Utc};
use oauth2::basic::BasicTokenResponse;
use oauth2::TokenResponse;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::Entity;

/// Public domain payload for a third-party connection (used for views and inputs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct ConnectionData {
    pub user_id: Uuid,
    pub org_id: Option<Uuid>,
    pub provider: String,
    pub provider_account_id: String,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub scopes: Vec<String>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub metadata: serde_json::Value,
}

impl ConnectionData {
    /// Checks whether this connection includes a specific granted scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Checks whether this connection includes all specified scopes.
    pub fn has_all_scopes(&self, scopes: &[&str]) -> bool {
        scopes.iter().all(|target| self.has_scope(target))
    }
}

/// OAuth 2.0 credential tokens stored in the database, isolated from public views.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct OAuth2Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_expires_at: Option<DateTime<Utc>>,
}

impl OAuth2Tokens {
    /// Checks if the access token has expired (or will expire within `margin_secs`).
    pub fn is_expired(&self, margin_secs: i64) -> bool {
        match self.token_expires_at {
            Some(expires_at) => Utc::now() + chrono::Duration::seconds(margin_secs) >= expires_at,
            None => false,
        }
    }
}

impl From<&BasicTokenResponse> for OAuth2Tokens {
    fn from(resp: &BasicTokenResponse) -> Self {
        let token_expires_at = resp.expires_in().map(|dur| {
            Utc::now() + chrono::Duration::from_std(dur).unwrap_or_default()
        });

        Self {
            access_token: resp.access_token().secret().to_string(),
            refresh_token: resp.refresh_token().map(|r| r.secret().to_string()),
            token_expires_at,
        }
    }
}

/// Combined connection payload and secret credentials.
///
/// Implements `Deref` to `ConnectionData` for direct field access.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct FullConnection {
    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub data: ConnectionData,

    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub tokens: OAuth2Tokens,
}

impl Deref for FullConnection {
    type Target = ConnectionData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for FullConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

/// Public connection view returned by API endpoints (no secrets).
pub type ConnectionView = Entity<Uuid, ConnectionData>;

/// Full persistent connection entity including tokens and database audit timestamps.
pub type Connection = Entity<Uuid, FullConnection>;

/// Input payload for upserting a connection (re-uses FullConnection).
pub type UpsertConnectionInput = FullConnection;

/// Extension methods on the full Connection entity.
pub trait ConnectionExt {
    /// Strips secret tokens and returns a public ConnectionView.
    fn to_view(&self) -> ConnectionView;
    /// Checks token expiration.
    fn is_expired(&self, margin_secs: i64) -> bool;
}

impl ConnectionExt for Connection {
    fn to_view(&self) -> ConnectionView {
        Entity::with_timestamps(self.id, self.data.data.clone(), self.created_at, self.updated_at)
    }

    fn is_expired(&self, margin_secs: i64) -> bool {
        self.data.tokens.is_expired(margin_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_connection_deref_and_view() {
        let id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let conn: Connection = Entity::new(
            id,
            FullConnection {
                data: ConnectionData {
                    user_id,
                    org_id: None,
                    provider: "google".to_string(),
                    provider_account_id: "google-1".to_string(),
                    scopes: vec!["openid".to_string(), "email".to_string()],
                    metadata: serde_json::json!({ "name": "Test" }),
                },
                tokens: OAuth2Tokens {
                    access_token: "secret-token".to_string(),
                    refresh_token: Some("secret-refresh".to_string()),
                    token_expires_at: None,
                },
            },
        );

        // Derefs through Entity -> FullConnection -> ConnectionData
        assert_eq!(conn.provider, "google");
        assert_eq!(conn.user_id, user_id);
        assert!(conn.has_scope("openid"));
        assert!(!conn.has_scope("calendar"));

        // to_view strips secret tokens
        let view = conn.to_view();
        assert_eq!(view.id, id);
        assert_eq!(view.provider, "google");

        let view_json = serde_json::to_string(&view).unwrap();
        assert!(!view_json.contains("secret-token"));
        assert!(!view_json.contains("secret-refresh"));
        assert!(view_json.contains("google-1"));
    }
}
