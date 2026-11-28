//! What a pure format returns: the file stays, gets new bytes, or goes.

/// The outcome of rendering one harness file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// The file already holds what Atlas wants. Don't touch it.
    Unchanged,
    /// Write these bytes in place of the current file (or create it).
    Write(Vec<u8>),
    /// Delete the file: Atlas created it, and nothing is left in it.
    Remove,
}

/// The current file can't be updated safely, so Atlas leaves it alone.
#[derive(Debug, thiserror::Error)]
pub enum FormatError {
    #[error("the file isn't valid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("the file isn't valid JSONC: expected {expected} at line {line}, column {column}")]
    InvalidJsonc {
        line: usize,
        column: usize,
        expected: &'static str,
    },
    #[error("the file isn't valid TOML: {0}")]
    InvalidToml(#[from] toml_edit::TomlError),
    #[error("the file isn't UTF-8 text")]
    NotText(#[from] std::str::Utf8Error),
    #[error("expected {expected} at {at}")]
    UnexpectedShape { at: String, expected: &'static str },
}

/// The setting at `at` isn't `expected`.
pub(crate) fn shape(at: &str, expected: &'static str) -> FormatError {
    FormatError::UnexpectedShape {
        at: at.to_owned(),
        expected,
    }
}
