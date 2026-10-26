//! Tenants and everything inside them. The data types stay plain values; this holds the injected pool.

use sqlx::PgPool;
use uuid::Uuid;

use super::Member;

/// Failure of a tenant operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TenantError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The tenant aggregate: tenants with their members, grants, invitations and auto-join domains.
#[derive(Clone)]
pub struct Tenants {
    pool: PgPool,
}

impl Tenants {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// All memberships of a user, earliest first; none for a deleted user.
    pub async fn memberships(&self, user_id: Uuid) -> Result<Vec<Member>, TenantError> {
        Ok(sqlx::query_as::<_, Member>("SELECT * FROM tenant_members WHERE user_id = $1 AND NOT EXISTS (SELECT 1 FROM users WHERE id = $1 AND deleted_at IS NOT NULL) ORDER BY joined_at ASC")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?)
    }
}
