//! Generic persistent entity wrapper providing ID and audit timestamps.

use std::ops::{Deref, DerefMut};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

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
