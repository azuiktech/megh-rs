//! Topic representation and hierarchical pattern matching.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::ops::Deref;

/// Represents an event topic or subscription pattern.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Topic(String);

impl Topic {
    /// Creates a new topic.
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    /// Evaluates whether the topic satisfies the given subscription pattern.
    ///
    /// Supported wildcards:
    /// - `*` matches exactly one path segment (e.g. `courses/*/chat` matches `courses/123/chat`).
    /// - `**` matches zero or more path segments (e.g. `courses/**` matches `courses/123/chat/read`).
    pub fn matches(&self, pattern: &Self) -> bool {
        if pattern.0.is_empty() || self.0.is_empty() {
            return pattern.0 == self.0;
        }
        if pattern.0 == "*" || pattern.0 == "**" {
            return true;
        }
        let pat_parts: Vec<&str> = pattern.0.split('/').collect();
        let top_parts: Vec<&str> = self.0.split('/').collect();
        match_segments(&pat_parts, &top_parts)
    }
}

fn match_segments(pat: &[&str], top: &[&str]) -> bool {
    match (pat.first(), top.first()) {
        (None, None) => true,
        (Some(&"**"), _) if pat.len() == 1 => true,
        (Some(&"**"), _) => (0..=top.len()).any(|i| match_segments(&pat[1..], &top[i..])),
        (Some(_), None) => false,
        (Some(&"*"), Some(_)) => match_segments(&pat[1..], &top[1..]),
        (Some(p), Some(t)) if p == t => match_segments(&pat[1..], &top[1..]),
        _ => false,
    }
}

impl Deref for Topic {
    type Target = str;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl fmt::Display for Topic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<S: Into<String>> From<S> for Topic {
    fn from(s: S) -> Self {
        Self(s.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_matching() {
        let topic = Topic::new("courses/123/chat");
        assert!(topic.matches(&Topic::new("courses/123/chat")));
        assert!(!topic.matches(&Topic::new("courses/456/chat")));
    }

    #[test]
    fn test_single_segment_wildcard() {
        let topic = Topic::new("courses/123/chat");
        assert!(topic.matches(&Topic::new("courses/*/chat")));
        assert!(!topic.matches(&Topic::new("courses/*")));
    }

    #[test]
    fn test_multi_segment_wildcard() {
        let topic = Topic::new("courses/123/chat/read");
        assert!(topic.matches(&Topic::new("courses/**")));
        assert!(topic.matches(&Topic::new("**")));
        assert!(!topic.matches(&Topic::new("users/**")));
    }
}
