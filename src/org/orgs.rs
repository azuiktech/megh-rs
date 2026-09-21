//! Organizations and everything inside them. The data types stay plain values; this holds the injected pool.

use sqlx::PgPool;
use uuid::Uuid;

use super::Member;

/// Failure of an organization operation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OrgError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// The organization aggregate: orgs with their members, grants, invitations and auto-join domains.
#[derive(Clone)]
pub struct Orgs {
    pool: PgPool,
}

impl Orgs {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// All memberships of a user, earliest first.
    pub async fn memberships(&self, user_id: Uuid) -> Result<Vec<Member>, OrgError> {
        Ok(sqlx::query_as::<_, Member>("SELECT * FROM organization_members WHERE user_id = $1 ORDER BY joined_at ASC")
            .bind(user_id)
            .fetch_all(&self.pool)
            .await?)
    }
}
