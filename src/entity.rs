//! Generic persistent entity wrapper providing ID and audit timestamps.

use std::ops::{Deref, DerefMut};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Trait for types that map to a database table with an explicit column list.
pub trait Table {
    const TABLE_NAME: &'static str;
    const COLUMNS: &'static [&'static str];
}

/// Generic persistent entity wrapper providing ID and audit timestamps.
///
/// Implements `Deref` and `DerefMut` to provide struct-embedding ergonomics,
/// while `#[serde(flatten)]` and `#[sqlx(flatten)]` preserve a flat representation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Entity<ID, T> {
    pub id: ID,
    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub data: T,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<ID, T> Deref for Entity<ID, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<ID, T> DerefMut for Entity<ID, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<ID, T> Entity<ID, T> {
    /// Creates a new entity with current UTC timestamps.
    pub fn new(id: ID, data: T) -> Self {
        let now = Utc::now();
        Self {
            id,
            data,
            created_at: now,
            updated_at: now,
        }
    }

    /// Creates an entity with explicit timestamps.
    pub fn with_timestamps(id: ID, data: T, created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> Self {
        Self {
            id,
            data,
            created_at,
            updated_at,
        }
    }

    /// Transforms the inner data while preserving the entity's ID and timestamps.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Entity<ID, U> {
        Entity {
            id: self.id,
            data: f(self.data),
            created_at: self.created_at,
            updated_at: self.updated_at,
        }
    }

    /// Merges an RFC 7396 JSON merge patch into this entity's data.
    pub fn patch(&self, patch: &serde_json::Value) -> Result<Self, serde_json::Error>
    where
        T: Serialize + for<'de> Deserialize<'de>,
        ID: Clone,
    {
        let mut target = serde_json::to_value(&self.data)?;
        json_patch::merge(&mut target, patch);
        let updated_data: T = serde_json::from_value(target)?;

        Ok(Entity {
            id: self.id.clone(),
            data: updated_data,
            created_at: self.created_at,
            updated_at: Utc::now(),
        })
    }

    #[cfg(feature = "postgres")]
    /// Inserts this entity into its table using json_populate_record.
    pub async fn insert(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Serialize + Send + Unpin,
        T: Table,
    {
        self.insert_into(pool, T::TABLE_NAME).await
    }

    #[cfg(feature = "postgres")]
    /// Inserts this entity into the specified table using json_populate_record.
    pub async fn insert_into(&self, pool: &sqlx::PgPool, table: &str) -> Result<Self, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Serialize + Send + Unpin,
    {
        let query = format!("INSERT INTO {table} SELECT * FROM json_populate_record(NULL::{table}, $1) RETURNING *");
        sqlx::query_as::<_, Self>(&query)
            .bind(sqlx::types::Json(self))
            .fetch_one(pool)
            .await
    }

    #[cfg(feature = "postgres")]
    /// Updates this entity in the specified table by ID using json_populate_record.
    pub async fn save(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Serialize + Send + Unpin,
        T: Table,
        ID: sqlx::Type<sqlx::Postgres> + for<'q> sqlx::Encode<'q, sqlx::Postgres> + Send + Sync + Clone,
    {
        let table = T::TABLE_NAME;
        let cols = T::COLUMNS.join(", ");
        let p_cols = T::COLUMNS.iter().map(|c| format!("p.{c}")).collect::<Vec<_>>().join(", ");
        let query = format!(
            "UPDATE {table} SET ({cols}) = ({p_cols}) FROM json_populate_record(NULL::{table}, $1) p WHERE {table}.id = $2 RETURNING {table}.*"
        );
        sqlx::query_as::<_, Self>(&query)
            .bind(sqlx::types::Json(self))
            .bind(self.id.clone())
            .fetch_one(pool)
            .await
    }

    #[cfg(feature = "postgres")]
    /// Finds an entity by ID using its associated Table::TABLE_NAME.
    pub async fn find_by_id(pool: &sqlx::PgPool, id: ID) -> Result<Option<Self>, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
        T: Table,
        ID: sqlx::Type<sqlx::Postgres> + for<'q> sqlx::Encode<'q, sqlx::Postgres> + Send + Sync,
    {
        let query = format!("SELECT * FROM {} WHERE id = $1", T::TABLE_NAME);
        sqlx::query_as::<_, Self>(&query)
            .bind(id)
            .fetch_optional(pool)
            .await
    }

    #[cfg(feature = "postgres")]
    /// Deletes an entity by ID using its associated Table::TABLE_NAME.
    pub async fn delete_by_id(pool: &sqlx::PgPool, id: ID) -> Result<bool, sqlx::Error>
    where
        T: Table,
        ID: sqlx::Type<sqlx::Postgres> + for<'q> sqlx::Encode<'q, sqlx::Postgres> + Send + Sync,
    {
        let query = format!("DELETE FROM {} WHERE id = $1", T::TABLE_NAME);
        let result = sqlx::query(&query)
            .bind(id)
            .execute(pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct SampleData {
        name: String,
        count: u32,
    }

    #[test]
    fn test_entity_deref_and_serde_flat() {
        let id = Uuid::new_v4();
        let entity = Entity::new(
            id,
            SampleData {
                name: "Alpha".to_string(),
                count: 42,
            },
        );

        // Deref directly into inner data fields
        assert_eq!(entity.name, "Alpha");
        assert_eq!(entity.count, 42);

        // Flat serialization
        let json = serde_json::to_string(&entity).unwrap();
        assert!(json.contains(&format!(r#""id":"{}""#, id)));
        assert!(json.contains(r#""name":"Alpha""#));
        assert!(json.contains(r#""count":42"#));
        assert!(!json.contains(r#""data":{"#)); // Flat, no nested data key

        let deserialized: Entity<Uuid, SampleData> = serde_json::from_str(&json).unwrap();
        assert_eq!(entity, deserialized);
    }
}
