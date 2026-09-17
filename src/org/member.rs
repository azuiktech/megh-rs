//! Organization membership and PBAC grant evaluation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use crate::auth::Grant;

/// Organization member association stored in table `org_members`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct OrgMember {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub user_id: Uuid,
    pub joined_at: DateTime<Utc>,
    pub grants: Vec<String>,
}

impl OrgMember {
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
        let member = OrgMember {
            id: Uuid::new_v4(),
            organization_id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            joined_at: Utc::now(),
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
