//! Organization tenant entity.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Organization tenant stored in table `org`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Org {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub timezone: String,
    pub created_at: DateTime<Utc>,
}
