//! Persistence repository for Connection entities in PostgreSQL.

use uuid::Uuid;

use super::model::{Connection, UpsertConnectionInput};

#[cfg(feature = "postgres")]
/// Connection repository for PostgreSQL operations.
pub struct ConnectionRepo<'a> {
    pool: &'a sqlx::PgPool,
}

#[cfg(feature = "postgres")]
impl<'a> ConnectionRepo<'a> {
    pub fn new(pool: &'a sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Fetches a connection by its UUID.
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<Connection>, sqlx::Error> {
        sqlx::query_as::<_, Connection>("SELECT * FROM connections WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool)
            .await
    }

    /// Finds a specific connection for a user, provider, and external account ID.
    pub async fn find_by_user_and_provider(
        &self,
        user_id: Uuid,
        provider: &str,
        provider_account_id: &str,
    ) -> Result<Option<Connection>, sqlx::Error> {
        sqlx::query_as::<_, Connection>(
            "SELECT * FROM connections WHERE user_id = $1 AND provider = $2 AND provider_account_id = $3",
        )
        .bind(user_id)
        .bind(provider)
        .bind(provider_account_id)
        .fetch_optional(self.pool)
        .await
    }

    /// Lists all connections for a specific user.
    pub async fn list_by_user(&self, user_id: Uuid) -> Result<Vec<Connection>, sqlx::Error> {
        sqlx::query_as::<_, Connection>(
            "SELECT * FROM connections WHERE user_id = $1 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .fetch_all(self.pool)
        .await
    }

    /// Lists all connections for a specific user filtered by provider.
    pub async fn list_by_user_and_provider(
        &self,
        user_id: Uuid,
        provider: &str,
    ) -> Result<Vec<Connection>, sqlx::Error> {
        sqlx::query_as::<_, Connection>(
            "SELECT * FROM connections WHERE user_id = $1 AND provider = $2 ORDER BY created_at DESC",
        )
        .bind(user_id)
        .bind(provider)
        .fetch_all(self.pool)
        .await
    }

    /// Upserts a connection: updates tokens, scopes, and metadata on conflict.
    pub async fn upsert(&self, input: &UpsertConnectionInput) -> Result<Connection, sqlx::Error> {
        sqlx::query_as::<_, Connection>(
            r#"
            INSERT INTO connections (
                user_id, org_id, provider, provider_account_id,
                access_token, refresh_token, token_expires_at, scopes, metadata, updated_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, NOW())
            ON CONFLICT (user_id, provider, provider_account_id) DO UPDATE SET
                org_id = COALESCE(EXCLUDED.org_id, connections.org_id),
                access_token = EXCLUDED.access_token,
                refresh_token = COALESCE(EXCLUDED.refresh_token, connections.refresh_token),
                token_expires_at = EXCLUDED.token_expires_at,
                scopes = EXCLUDED.scopes,
                metadata = EXCLUDED.metadata,
                updated_at = NOW()
            RETURNING *
            "#,
        )
        .bind(input.user_id)
        .bind(input.org_id)
        .bind(&input.provider)
        .bind(&input.provider_account_id)
        .bind(&input.tokens.access_token)
        .bind(&input.tokens.refresh_token)
        .bind(input.tokens.token_expires_at)
        .bind(serde_json::to_value(&input.scopes).unwrap_or_default())
        .bind(&input.metadata)
        .fetch_one(self.pool)
        .await
    }

    /// Deletes a connection by its ID.
    pub async fn delete(&self, id: Uuid) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM connections WHERE id = $1")
            .bind(id)
            .execute(self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_connection_repo_construction() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
        let repo = ConnectionRepo::new(&pool);
        assert_eq!(repo.pool.size(), 0);
    }
}
