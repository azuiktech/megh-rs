//! User identity models and persistence repository using Entity<ID, T>.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::entity::Entity;

/// User profile business fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct UserProfile {
    pub account_id: Option<String>,
    pub provider: Option<String>,
    pub email: String,
    pub display_name: String,
    pub photo_url: String,
}

/// Full user entity wrapping UserProfile with ID and audit timestamps.
pub type User = Entity<Uuid, UserProfile>;

/// Parameters for creating or updating a user identity upon login.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertUserInput {
    pub provider: String,
    pub account_id: String,
    pub email: String,
    pub display_name: Option<String>,
    pub photo_url: Option<String>,
}

#[cfg(feature = "postgres")]
/// User repository for PostgreSQL operations.
pub struct UserRepo<'a> {
    pool: &'a sqlx::PgPool,
}

#[cfg(feature = "postgres")]
impl<'a> UserRepo<'a> {
    pub fn new(pool: &'a sqlx::PgPool) -> Self {
        Self { pool }
    }

    /// Fetches a user by UUID.
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = $1")
            .bind(id)
            .fetch_optional(self.pool)
            .await
    }

    /// Finds a user by email.
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = $1")
            .bind(email)
            .fetch_optional(self.pool)
            .await
    }

    /// Finds a user by the identity of the provider that first created it.
    pub async fn find_by_account(&self, provider: &str, account_id: &str) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE provider = $1 AND account_id = $2")
            .bind(provider)
            .bind(account_id)
            .fetch_optional(self.pool)
            .await
    }

    /// Upserts user by email: one user across providers. The first provider identity is kept (only filled in
    /// when unset); non-empty display name/photo are updated; inserts if not existing.
    pub async fn upsert(&self, input: &UpsertUserInput) -> Result<User, sqlx::Error> {
        let display_name = input.display_name.as_deref().unwrap_or("");
        let photo_url = input.photo_url.as_deref().unwrap_or("");

        sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (provider, account_id, email, display_name, photo_url, updated_at)
            VALUES ($1, $2, $3, $4, $5, NOW())
            ON CONFLICT (email) DO UPDATE SET
                provider = COALESCE(users.provider, EXCLUDED.provider),
                account_id = COALESCE(users.account_id, EXCLUDED.account_id),
                display_name = CASE WHEN EXCLUDED.display_name <> '' THEN EXCLUDED.display_name ELSE users.display_name END,
                photo_url = CASE WHEN EXCLUDED.photo_url <> '' THEN EXCLUDED.photo_url ELSE users.photo_url END,
                updated_at = NOW()
            RETURNING *
            "#,
        )
        .bind(&input.provider)
        .bind(&input.account_id)
        .bind(&input.email)
        .bind(display_name)
        .bind(photo_url)
        .fetch_one(self.pool)
        .await
    }

    /// Updates display name and photo URL for an existing user.
    pub async fn update_profile(
        &self,
        id: Uuid,
        display_name: &str,
        photo_url: &str,
    ) -> Result<User, sqlx::Error> {
        sqlx::query_as::<_, User>(
            r#"
            UPDATE users
            SET display_name = $2, photo_url = $3, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(display_name)
        .bind(photo_url)
        .fetch_one(self.pool)
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_entity_deref_and_serialization() {
        let id = Uuid::parse_str("a0000000-0000-0000-0000-000000000001").unwrap();
        let user: User = Entity::new(
            id,
            UserProfile {
                account_id: Some("1234".to_string()),
                provider: Some("google".to_string()),
                email: "abirbasak@gmail.com".to_string(),
                display_name: "Abir Basak".to_string(),
                photo_url: "https://photo.example.com/avatar.png".to_string(),
            },
        );

        // Deref directly into profile fields
        assert_eq!(user.id, id);
        assert_eq!(user.provider.as_deref(), Some("google"));
        assert_eq!(user.email, "abirbasak@gmail.com");
        assert_eq!(user.display_name, "Abir Basak");

        // Flat JSON serialization without nested data key
        let json = serde_json::to_string(&user).unwrap();
        assert!(json.contains("abirbasak@gmail.com"));
        assert!(!json.contains(r#""data":{"#));

        let deserialized: User = serde_json::from_str(&json).unwrap();
        assert_eq!(user, deserialized);
    }

    #[test]
    fn test_upsert_input_serialization() {
        let input = UpsertUserInput {
            provider: "google".to_string(),
            account_id: "1234".to_string(),
            email: "test@example.com".to_string(),
            display_name: Some("Test User".to_string()),
            photo_url: None,
        };

        let json = serde_json::to_string(&input).unwrap();
        assert!(json.contains("test@example.com"));
        assert!(json.contains("Test User"));
    }

    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn test_user_repo_construction() {
        let pool = sqlx::PgPool::connect_lazy("postgres://localhost/test").unwrap();
        let repo = UserRepo::new(&pool);
        assert_eq!(repo.pool.size(), 0);
    }
}
