//! Apache Shiro style Policy-Based Access Control Grant matching.

use serde::{Deserialize, Serialize};

/// Apache Shiro format Grant ("resource:action:instance").
/// Supports wildcards (`*`) and subparts separated by commas (`,`).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Grant(pub String);

impl Grant {
    /// Creates a new Grant from a string representation.
    pub fn new(permission: impl Into<String>) -> Self {
        Self(permission.into().trim().to_lowercase())
    }

    /// Returns the raw permission string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Evaluates if this grant implies the required target grant.
    ///
    /// Rules:
    /// - Global wildcard `"*"` implies all permissions.
    /// - An empty grant never implies any target (unless target is also empty).
    /// - Matching is done segment-by-segment separated by `':'`.
    /// - A wildcard segment `"*"` implies any target segment at that position.
    /// - Comma-separated subparts (e.g. `"read,write"`) match if any subpart matches the target.
    pub fn implies(&self, target: &Grant) -> bool {
        if self.0 == "*" {
            return true;
        }
        if self.0.is_empty() || target.0.is_empty() {
            return self.0 == target.0;
        }

        let self_parts: Vec<&str> = self.0.split(':').collect();
        let target_parts: Vec<&str> = target.0.split(':').collect();

        // If self has more parts than target, it cannot imply target unless trailing parts are wildcards
        if self_parts.len() > target_parts.len() {
            return false;
        }

        self_parts
            .iter()
            .enumerate()
            .all(|(idx, &self_part)| {
                let target_part = target_parts[idx];
                self_part == "*" || self_part.split(',').any(|sp| sp == "*" || sp == target_part)
            })
            && (self_parts.len() == target_parts.len() || self_parts.last() == Some(&"*"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_match() {
        let g = Grant::new("course:read");
        assert!(g.implies(&Grant::new("course:read")));
        assert!(!g.implies(&Grant::new("course:write")));
        assert!(!g.implies(&Grant::new("profile:read")));
    }

    #[test]
    fn test_global_wildcard() {
        let g = Grant::new("*");
        assert!(g.implies(&Grant::new("course:read")));
        assert!(g.implies(&Grant::new("profile:write:123")));
        assert!(g.implies(&Grant::new("anything")));
    }

    #[test]
    fn test_action_wildcard() {
        let g = Grant::new("course:*");
        assert!(g.implies(&Grant::new("course:read")));
        assert!(g.implies(&Grant::new("course:write")));
        assert!(g.implies(&Grant::new("course:publish")));
        assert!(!g.implies(&Grant::new("episode:read")));
    }

    #[test]
    fn test_comma_separated_subparts() {
        let g = Grant::new("course:read,write");
        assert!(g.implies(&Grant::new("course:read")));
        assert!(g.implies(&Grant::new("course:write")));
        assert!(!g.implies(&Grant::new("course:publish")));
    }

    #[test]
    fn test_case_insensitivity_and_trimming() {
        let g = Grant::new(" Course:Read ");
        assert!(g.implies(&Grant::new("course:read")));
    }
}
