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
    #[error("expected {expected} at {at}")]
    UnexpectedShape { at: String, expected: &'static str },
}
