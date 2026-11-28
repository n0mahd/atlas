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

/// Whether a file is a JSON object laid out exactly as the formats here write one, or
/// blank. Rewriting such a file changes only the entries a format edits; any other file
/// (its own spacing or a repeated key) loses some of its bytes when it is rendered again.
#[must_use]
pub fn in_atlas_layout(bytes: &[u8]) -> bool {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return true;
    }
    let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(bytes) else {
        return false;
    };
    serde_json::to_vec_pretty(&map).is_ok_and(|mut b| {
        b.push(b'\n');
        b == bytes
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_atlas_s_own_layout_survives_a_rewrite_byte_for_byte() {
        assert!(in_atlas_layout(b""));
        assert!(in_atlas_layout(
            b"{\n  \"b\": 1,\n  \"a\": [\n    true\n  ]\n}\n"
        ));
        assert!(!in_atlas_layout(b"{\"b\": 1}\n"), "its own spacing");
        assert!(!in_atlas_layout(b"{\n  \"b\": 1\n}"), "no final newline");
        assert!(
            !in_atlas_layout(b"{\n  \"b\": 1,\n  \"b\": 2\n}\n"),
            "a repeated key"
        );
        assert!(!in_atlas_layout(b"[]\n"));
        assert!(!in_atlas_layout(b"// note\n{}\n"));
    }
}
