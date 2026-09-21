//! PostgreSQL repository for ConnectedAccount persistence.

use sqlx::PgPool;
use super::model::ConnectedAccount;

#[cfg(feature = "postgres")]
/// Repository for `connected_accounts` table operations in PostgreSQL.
pub struct ConnectedAccountRepo<'a> {
    pool: &'a PgPool,
}

#[cfg(feature = "postgres")]
impl<'a> ConnectedAccountRepo<'a> {
    pub fn new(pool: &'a PgPool) -> Self {
        Self { pool }
    }

    /// Fetches an account by exact compound primary key (account_id, provider).
    pub async fn get(&self, account_id: &str, provider: &str) -> Result<Option<ConnectedAccount>, sqlx::Error> {
        sqlx::query_as::<_, ConnectedAccount>(
            "SELECT * FROM connected_accounts WHERE account_id = $1 AND provider = $2"
        )
        .bind(account_id)
        .bind(provider)
        .fetch_optional(self.pool)
        .await
    }

    /// Finds an account matching identifier as either account_id or email for the provider.
    pub async fn find_by_email_or_account(
        &self,
        identifier: &str,
        provider: &str,
    ) -> Result<Option<ConnectedAccount>, sqlx::Error> {
        sqlx::query_as::<_, ConnectedAccount>(
            "SELECT * FROM connected_accounts WHERE (account_id = $1 OR email = $1) AND provider = $2 AND disconnected_at IS NULL LIMIT 1"
        )
        .bind(identifier)
        .bind(provider)
        .fetch_optional(self.pool)
        .await
    }

    /// Saves or updates a connected account. A missing refresh token keeps the stored one (providers only send it on first consent).
    pub async fn save(&self, account: &ConnectedAccount) -> Result<ConnectedAccount, sqlx::Error> {
        sqlx::query_as::<_, ConnectedAccount>(
            "INSERT INTO connected_accounts (account_id, provider, email, access_token, refresh_token, token_type, expiry, disconnected_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
             ON CONFLICT (account_id, provider) DO UPDATE SET
                 email = EXCLUDED.email,
                 access_token = EXCLUDED.access_token,
                 refresh_token = COALESCE(EXCLUDED.refresh_token, connected_accounts.refresh_token),
                 token_type = EXCLUDED.token_type,
                 expiry = EXCLUDED.expiry,
                 disconnected_at = EXCLUDED.disconnected_at,
                 updated_at = NOW()
             RETURNING *",
        )
        .bind(&account.account_id)
        .bind(&account.provider)
        .bind(&account.email)
        .bind(&account.access_token)
        .bind(&account.refresh_token)
        .bind(&account.token_type)
        .bind(account.expiry)
        .bind(account.disconnected_at)
        .fetch_one(self.pool)
        .await
    }

    /// Soft-disconnects an account by setting disconnected_at to NOW().
    pub async fn disconnect(&self, account_id: &str, provider: &str) -> Result<bool, sqlx::Error> {
        let res = sqlx::query(
            "UPDATE connected_accounts SET disconnected_at = NOW(), updated_at = NOW() WHERE account_id = $1 AND provider = $2"
        )
        .bind(account_id)
        .bind(provider)
        .execute(self.pool)
        .await?;

        Ok(res.rows_affected() > 0)
    }

    /// Lists all active (non-disconnected) accounts associated with an email.
    pub async fn list_by_email(&self, email: &str) -> Result<Vec<ConnectedAccount>, sqlx::Error> {
        sqlx::query_as::<_, ConnectedAccount>(
            "SELECT * FROM connected_accounts WHERE email = $1 AND disconnected_at IS NULL ORDER BY created_at DESC"
        )
        .bind(email)
        .fetch_all(self.pool)
        .await
    }
}
