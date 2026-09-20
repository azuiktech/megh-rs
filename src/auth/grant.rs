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

    /// Creates a Grant from resource, action, and optional instance.
    pub fn from_parts(resource: &str, action: &str, instance: Option<&str>) -> Self {
        match instance {
            Some(inst) => Self::new(format!("{}:{}:{}", resource, action, inst)),
            None => Self::new(format!("{}:{}", resource, action)),
        }
    }

    /// Returns the raw permission string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the resource segment if present.
    pub fn resource(&self) -> &str {
        self.0.split(':').next().unwrap_or("")
    }

    /// Returns the action segment if present.
    pub fn action(&self) -> Option<&str> {
        self.0.split(':').nth(1)
    }

    /// Returns the instance segment if present.
    pub fn instance(&self) -> Option<&str> {
        self.0.split(':').nth(2)
    }

    /// Evaluates if this grant implies the required target grant.
    ///
    /// Rules (Apache Shiro format):
    /// - Global wildcard `"*"` implies all permissions.
    /// - Empty grant never implies any target (unless target is also empty).
    /// - If granted has more parts than target, it cannot imply target unless trailing parts are `*`.
    /// - For each part in granted: `*` matches anything, comma-separated subparts match any subpart.
    /// - If granted runs out of parts, remaining target parts are implicitly satisfied.
    pub fn implies(&self, target: &Grant) -> bool {
        if self.0 == "*" {
            return true;
        }
        if self.0.is_empty() || target.0.is_empty() {
            return self.0 == target.0;
        }

        let self_parts: Vec<&str> = self.0.split(':').collect();
        let target_parts: Vec<&str> = target.0.split(':').collect();

        if self_parts.len() > target_parts.len()
            && !self_parts[target_parts.len()..].iter().all(|&p| p == "*")
        {
            return false;
        }

        self_parts
            .iter()
            .take(target_parts.len())
            .enumerate()
            .all(|(idx, &self_part)| {
                let target_part = target_parts[idx];
                self_part == "*" || self_part.split(',').any(|sp| sp == "*" || sp == target_part)
            })
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
