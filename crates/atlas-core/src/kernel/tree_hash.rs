use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A git tree object id: 40 (SHA-1) or 64 (SHA-256) lowercase hex digits.
///
/// A release pins one, and a client checks the tree it fetched against it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct TreeHash(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid git tree hash {0:?}: expected 40 or 64 lowercase hex digits")]
pub struct InvalidTreeHash(String);

impl TreeHash {
    /// Validates and wraps a tree hash.
    ///
    /// # Errors
    /// Returns [`InvalidTreeHash`] unless the value is 40 or 64 lowercase hex digits.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidTreeHash> {
        let value = value.into();
        let hex = value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if hex && matches!(value.len(), 40 | 64) {
            Ok(Self(value))
        } else {
            Err(InvalidTreeHash(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TreeHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for TreeHash {
    type Err = InvalidTreeHash;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for TreeHash {
    type Error = InvalidTreeHash;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<TreeHash> for String {
    fn from(hash: TreeHash) -> Self {
        hash.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_sha1_and_sha256() {
        assert!(TreeHash::new("4b825dc642cb6eb9a060e54bf8d69288fbee4904").is_ok());
        assert!(TreeHash::new("a".repeat(64)).is_ok());
    }

    #[test]
    fn rejects_other_shapes() {
        for bad in [
            String::new(),
            "4B825DC642CB6EB9A060E54BF8D69288FBEE4904".into(),
            "4b825dc".into(),
            "g".repeat(40),
            "a".repeat(41),
        ] {
            assert!(TreeHash::new(bad.clone()).is_err(), "{bad}");
        }
    }
}
