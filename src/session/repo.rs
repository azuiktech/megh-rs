//! PostgreSQL persistence repository for Session entities.

use chrono::{Duration, Utc};
use uuid::Uuid;

use super::model::{
    generate_session_token, hash_session_token, CreatedSession, Session,
};

#[cfg(feature = "postgres")]
/// Session repository for PostgreSQL operations.
pub struct SessionRepo<'a> {
    pool: &'a sqlx::PgPool,
}

#[cfg(feature = "postgres")]
impl<'a> SessionRepo<'a> {
    pub fn new(pool: &'a sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Creates a new session with a generated cryptographically secure plaintext token.
    pub async fn create(
        &self,
        user_id: Uuid,
        duration: Duration,
        user_agent: &str,
        ip_address: &str,
        metadata: serde_json::Value,
    ) -> Result<CreatedSession, sqlx::Error> {
        let plaintext_token = generate_session_token();
        let token_hash = hash_session_token(&plaintext_token);
        let expires_at = Utc::now() + duration;

        let session = sqlx::query_as::<_, Session>(
            r#"
            INSERT INTO sessions (user_id, token_hash, expires_at, user_agent, ip_address, metadata)
            VALUES ($1, $2, $3, $4, $5, $6)
            RETURNING *
            "#,
        )
        .bind(user_id)
        .bind(&token_hash)
        .bind(expires_at)
        .bind(user_agent)
        .bind(ip_address)
        .bind(&metadata)
        .fetch_one(self.pool)
        .await?;

        Ok(CreatedSession {
            session,
            plaintext_token,
        })
    }

    /// Finds an active, non-expired session by its plaintext token.
    pub async fn find_valid_by_token(
        &self,
        plaintext_token: &str,
    ) -> Result<Option<Session>, sqlx::Error> {
        let token_hash = hash_session_token(plaintext_token);

        sqlx::query_as::<_, Session>(
            "SELECT * FROM sessions WHERE token_hash = $1 AND expires_at > NOW()",
        )
        .bind(&token_hash)
        .fetch_optional(self.pool)
        .await
    }

    /// Extends the expiration of an existing session.
    pub async fn touch(&self, id: Uuid, extension: Duration) -> Result<Option<Session>, sqlx::Error> {
        let extension_secs = extension.num_seconds();

        sqlx::query_as::<_, Session>(
            r#"
            UPDATE sessions
            SET expires_at = expires_at + ($2 * INTERVAL '1 second'), updated_at = NOW()
            WHERE id = $1 AND expires_at > NOW()
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(extension_secs)
        .fetch_optional(self.pool)
        .await
    }

    /// Revokes (deletes) a specific session by ID.
    pub async fn revoke(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Revokes (deletes) a specific session by plaintext token.
    pub async fn revoke_by_token(&self, plaintext_token: &str) -> Result<bool, sqlx::Error> {
        let token_hash = hash_session_token(plaintext_token);
        let result = sqlx::query("DELETE FROM sessions WHERE token_hash = $1")
            .bind(&token_hash)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    /// Revokes all active sessions for a given user.
    pub async fn revoke_all_for_user(&self, user_id: Uuid) -> Result<u64, sqlx::Error> {
        let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
            .bind(user_id)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    /// Lists all sessions for a specific user.
    pub async fn list_by_user(&self, user_id: Uuid) -> Result<Vec<Session>, sqlx::Error> {
        sqlx::query_as::<_, Session>(
            "SELECT * FROM sessions WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(self.pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_session_repo_construction() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
        let repo = SessionRepo::new(&pool);
        assert_eq!(repo.pool.size(), 0);
    }
}
