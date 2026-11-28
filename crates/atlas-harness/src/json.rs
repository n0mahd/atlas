//! Strict JSON object files, as the hook, instruction and switch formats edit them:
//! parse the whole object, edit it, and render it the way the harnesses write it.

use serde_json::{Map, Value};

use crate::change::{Change, FormatError, shape};

/// Parses a JSON object file. A missing or blank file is an empty object.
pub(crate) fn parse_object(current: Option<&[u8]>) -> Result<Map<String, Value>, FormatError> {
    match current {
        Some(bytes) if !bytes.iter().all(u8::is_ascii_whitespace) => {
            match serde_json::from_slice(bytes)? {
                Value::Object(map) => Ok(map),
                _ => Err(shape("the top level", "an object")),
            }
        }
        _ => Ok(Map::new()),
    }
}

/// Compares the edited object with the original and renders the outcome: two-space
/// JSON with a trailing newline, the layout the harnesses write themselves; an object
/// left empty means the file goes.
pub(crate) fn finish(
    original: &Map<String, Value>,
    edited: &Map<String, Value>,
) -> Result<Change, FormatError> {
    if edited == original {
        return Ok(Change::Unchanged);
    }
    if edited.is_empty() {
        return Ok(Change::Remove);
    }
    let mut bytes = serde_json::to_vec_pretty(edited)?;
    bytes.push(b'\n');
    Ok(Change::Write(bytes))
}
