//! Session domain models, token hashing, and Entity<ID, T> representation.

use std::ops::{Deref, DerefMut};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::Entity;

/// Public session data and client context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct SessionData {
    pub user_id: Uuid,
    pub expires_at: DateTime<Utc>,
    pub user_agent: String,
    pub ip_address: String,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub metadata: serde_json::Value,
}

impl SessionData {
    /// Checks whether the session has expired.
    pub fn is_expired(&self) -> bool {
        Utc::now() >= self.expires_at
    }
}

/// Sensitive session token hash stored in the database.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct SessionSecrets {
    pub token_hash: String,
}

/// Full persistent session combining metadata and token hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct FullSession {
    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub data: SessionData,

    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub secrets: SessionSecrets,
}

impl Deref for FullSession {
    type Target = SessionData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl DerefMut for FullSession {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

/// Public session view returned by API endpoints (no secret token hash).
pub type SessionView = Entity<Uuid, SessionData>;

/// Full persistent session entity stored in the database.
pub type Session = Entity<Uuid, FullSession>;

/// A newly created session paired with its plaintext secret token (only returned once upon creation).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedSession {
    pub session: Session,
    pub plaintext_token: String,
}

/// Hashes a plaintext session token using SHA-256 for secure storage at rest.
pub fn hash_session_token(token: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Generates a cryptographically random plaintext session token.
pub fn generate_session_token() -> String {
    format!("{}-{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// Extension methods for Session.
pub trait SessionExt {
    /// Strips token hash and returns safe SessionView.
    fn to_view(&self) -> SessionView;
}

impl SessionExt for Session {
    fn to_view(&self) -> SessionView {
        Entity::with_timestamps(self.id, self.data.data.clone(), self.created_at, self.updated_at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_hashing_deterministic() {
        let token = "my-secure-session-token-123";
        let hash1 = hash_session_token(token);
        let hash2 = hash_session_token(token);
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64); // SHA-256 hex string length
    }

    #[test]
    fn test_generate_session_token_unique() {
        let t1 = generate_session_token();
        let t2 = generate_session_token();
        assert_ne!(t1, t2);
        assert!(t1.len() >= 60);
    }

    #[test]
    fn test_session_deref_and_view() {
        let id = Uuid::new_v4();
        let user_id = Uuid::new_v4();
        let session: Session = Entity::new(
            id,
            FullSession {
                data: SessionData {
                    user_id,
                    expires_at: Utc::now() + chrono::Duration::hours(24),
                    user_agent: "Mozilla/5.0".to_string(),
                    ip_address: "127.0.0.1".to_string(),
                    metadata: serde_json::json!({ "theme": "dark" }),
                },
                secrets: SessionSecrets {
                    token_hash: "abcd1234hash".to_string(),
                },
            },
        );

        // Deref into SessionData
        assert_eq!(session.user_id, user_id);
        assert_eq!(session.user_agent, "Mozilla/5.0");
        assert!(!session.is_expired());

        // to_view strips secrets
        let view = session.to_view();
        let json = serde_json::to_string(&view).unwrap();
        assert!(!json.contains("abcd1234hash"));
        assert!(json.contains("127.0.0.1"));
    }
}
