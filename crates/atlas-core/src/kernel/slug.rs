use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

const MAX_LEN: usize = 64;

/// A lowercase kebab-case name, such as a skill, hook or project name.
///
/// Slugs become path segments, so they are restricted to `a-z`, `0-9` and single
/// hyphens between them. That rules out `..`, separators and hidden files by
/// construction.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Slug(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid name {value:?}: {reason}")]
pub struct InvalidSlug {
    value: String,
    reason: &'static str,
}

impl Slug {
    /// Validates and wraps a slug.
    ///
    /// # Errors
    /// Returns [`InvalidSlug`] unless the value is 1 to 64 characters of `a-z` and
    /// `0-9`, optionally joined by single hyphens.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidSlug> {
        let value = value.into();
        let fail = |reason| {
            Err(InvalidSlug {
                value: value.clone(),
                reason,
            })
        };
        if value.is_empty() {
            return fail("must not be empty");
        }
        if value.len() > MAX_LEN {
            return fail("must be at most 64 characters");
        }
        if !value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        {
            return fail("may contain only a-z, 0-9 and hyphens");
        }
        if value.starts_with('-') || value.ends_with('-') || value.contains("--") {
            return fail("hyphens must sit between letters or digits");
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Slug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for Slug {
    type Err = InvalidSlug;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for Slug {
    type Error = InvalidSlug;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<Slug> for String {
    fn from(slug: Slug) -> Self {
        slug.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_kebab_case() {
        for ok in [
            "a",
            "skill",
            "code-review",
            "mcp-2",
            "x".repeat(64).as_str(),
        ] {
            assert!(Slug::new(ok).is_ok(), "{ok}");
        }
    }

    #[test]
    fn rejects_anything_that_could_escape_a_path() {
        let too_long = "x".repeat(65);
        for bad in [
            "",
            "..",
            ".hidden",
            "a/b",
            "a\\b",
            "Skill",
            "a_b",
            "-a",
            "a-",
            "a--b",
            "a b",
            "é",
            too_long.as_str(),
        ] {
            assert!(Slug::new(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn deserialising_validates() {
        assert!(serde_json::from_str::<Slug>(r#""good-name""#).is_ok());
        assert!(serde_json::from_str::<Slug>(r#""../etc""#).is_err());
    }
}
