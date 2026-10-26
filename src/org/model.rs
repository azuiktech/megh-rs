//! Organization tenant entity.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::booking::value::{Address, Contact};

/// Organization tenant stored in table `organizations` (same columns as megh-go).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Org {
    pub id: Uuid,
    pub name: Option<String>,
    pub description: Option<String>,
    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub contact: Contact,
    #[serde(default)]
    #[cfg_attr(feature = "postgres", sqlx(json(nullable)))]
    pub address: Option<Address>,
    pub timezone: String,
    pub sub_status: String,
    pub trial_ends_at: Option<DateTime<Utc>>,
    pub plan_id: Option<Uuid>,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: Option<DateTime<Utc>>,
}
