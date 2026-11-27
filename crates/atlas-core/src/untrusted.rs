//! Content Atlas did not write: skill bodies, memory pages, instruction files.
//!
//! Any of it may be hostile (a poisoned skill in a project repo, say). Wrapping it means
//! the only way to put it on a page is through a renderer that sanitises it.

use std::fmt;

/// Text read from a file someone else can edit. Not `Display`: callers must choose to
/// read the raw text, and the web layer only does that inside its sanitiser.
#[derive(Clone, PartialEq, Eq)]
pub struct Untrusted<T>(T);

impl<T> Untrusted<T> {
    pub const fn new(value: T) -> Self {
        Self(value)
    }

    /// The raw value. Never send it to a browser without escaping or sanitising it.
    pub const fn raw(&self) -> &T {
        &self.0
    }
}

impl<T> fmt::Debug for Untrusted<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Untrusted(..)")
    }
}

/// Markdown source text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Markdown(pub String);

impl Markdown {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
