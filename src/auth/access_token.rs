//! The claims of megh's short-lived JWT access tokens (RFC 9068, the claims megh-go uses).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Grant;

/// `sub` is the member id, `aud` the tenant id and `scope` the member's grants, space separated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub sub: Uuid,
    pub iss: String,
    pub aud: Vec<String>,
    pub exp: u64,
    pub iat: u64,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub scope: String,
}

impl AccessToken {
    pub fn grants(&self) -> Vec<Grant> {
        self.scope.split_whitespace().map(Grant::new).collect()
    }
}
