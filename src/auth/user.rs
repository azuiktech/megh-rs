//! User identity models and persistence repository.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// User identity entity stored in table `users`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct User {
    pub id: Uuid,
    pub subject: String,
    pub email: String,
    pub display_name: String,
    pub photo_url: String,
    pub created_at: DateTime<Utc>,
}

/// Parameters for creating or updating a user identity upon login.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpsertUserInput {
    pub subject: String,
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

    /// Finds a user by OAuth provider subject.
    pub async fn find_by_subject(&self, subject: &str) -> Result<Option<User>, sqlx::Error> {
        sqlx::query_as::<_, User>("SELECT * FROM users WHERE subject = $1")
            .bind(subject)
            .fetch_optional(self.pool)
            .await
    }

    /// Upserts user by email: updates subject and non-empty display name/photo; inserts if not existing.
    pub async fn upsert(&self, input: &UpsertUserInput) -> Result<User, sqlx::Error> {
        let display_name = input.display_name.as_deref().unwrap_or("");
        let photo_url = input.photo_url.as_deref().unwrap_or("");

        sqlx::query_as::<_, User>(
            r#"
            INSERT INTO users (subject, email, display_name, photo_url)
            VALUES ($1, $2, $3, $4)
            ON CONFLICT (email) DO UPDATE SET
                subject = EXCLUDED.subject,
                display_name = CASE WHEN EXCLUDED.display_name <> '' THEN EXCLUDED.display_name ELSE users.display_name END,
                photo_url = CASE WHEN EXCLUDED.photo_url <> '' THEN EXCLUDED.photo_url ELSE users.photo_url END
            RETURNING *
            "#,
        )
        .bind(&input.subject)
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
            SET display_name = $2, photo_url = $3
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
    fn test_user_serialization() {
        let id = Uuid::parse_str("a0000000-0000-0000-0000-000000000001").unwrap();
        let created_at = DateTime::parse_from_rfc3339("2026-09-17T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);

        let user = User {
            id,
            subject: "google:abirbasak@gmail.com".to_string(),
            email: "abirbasak@gmail.com".to_string(),
            display_name: "Abir Basak".to_string(),
            photo_url: "https://photo.example.com/avatar.png".to_string(),
            created_at,
        };

        let json = serde_json::to_string(&user).unwrap();
        let deserialized: User = serde_json::from_str(&json).unwrap();
        assert_eq!(user, deserialized);
    }

    #[test]
    fn test_upsert_input_serialization() {
        let input = UpsertUserInput {
            subject: "google:test@example.com".to_string(),
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
