//! Organization membership and PBAC grant evaluation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::auth::Grant;

/// Organization member stored in table `organization_members` (same columns as megh-go).
/// `grants` is a JSON array of strings in a text column; NULL, `[]` and `null` all mean no grants.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Member {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub user_id: Uuid,
    pub role: String,
    pub grants: Vec<String>,
    pub joined_at: DateTime<Utc>,
    pub invited_by: Option<Uuid>,
}

#[cfg(feature = "postgres")]
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for Member {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        let grants: Option<String> = row.try_get("grants")?;
        let grants = grants
            .map_or(Ok(None), |text| serde_json::from_str::<Option<Vec<String>>>(&text))
            .map_err(|e| sqlx::Error::ColumnDecode { index: "grants".into(), source: Box::new(e) })?;
        Ok(Self {
            id: row.try_get("id")?,
            organization_id: row.try_get("organization_id")?,
            user_id: row.try_get("user_id")?,
            role: row.try_get("role")?,
            grants: grants.unwrap_or_default(),
            joined_at: row.try_get("joined_at")?,
            invited_by: row.try_get("invited_by")?,
        })
    }
}

impl Member {
    /// Returns true if any of the member's grants implies the required grant.
    pub fn has_grant(&self, required: &Grant) -> bool {
        self.grants
            .iter()
            .map(Grant::new)
            .any(|g| g.implies(required))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_org_member_has_grant() {
        let member = Member {
            id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            role: "member".to_string(),
            joined_at: Utc::now(),
            invited_by: None,
            grants: vec![
                "profile:read".to_string(),
                "course:*".to_string(),
            ],
        };

        assert!(member.has_grant(&Grant::new("profile:read")));
        assert!(member.has_grant(&Grant::new("course:read")));
        assert!(member.has_grant(&Grant::new("course:publish")));
        assert!(!member.has_grant(&Grant::new("profile:delete")));
        assert!(!member.has_grant(&Grant::new("admin:*")));
    }
}
