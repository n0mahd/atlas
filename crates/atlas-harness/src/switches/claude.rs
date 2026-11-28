//! Claude Code: `"skillOverrides": {"<name>": "off"}` in `settings.json` (home) or a
//! repo's `.claude/settings.local.json`.
//!
//! The file is strict JSON and the values are one of four states, so there is no room for
//! a marker on the entry itself. Atlas lists the names it switched off under its own
//! top-level key, [`RECORD`]; Claude Code accepts the extra key (checked with
//! `claude doctor` on 2.1.332). A name counts as Atlas's only while it is in that list and
//! its entry still says `"off"`: once the owner changes or removes the entry, it is theirs.

use std::collections::BTreeSet;

use serde_json::{Map, Value};

use super::{record, recorded};
use crate::change::shape;
use crate::json::{finish, parse_object};
use crate::{Change, FormatError};

/// Atlas's own key: the names whose `"off"` Atlas wrote.
pub(super) const RECORD: &str = "atlas:skillOverrides";

/// Every value Claude Code accepts. One unknown value makes it drop the whole map, so a
/// file holding one is refused rather than edited.
const STATES: [&str; 4] = ["on", "name-only", "user-invocable-only", "off"];

pub(super) fn render(
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
) -> Result<Change, FormatError> {
    let original = parse_object(current)?;
    let mut doc = original.clone();
    let mut overrides = match doc.get("skillOverrides") {
        None => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => return Err(shape("skillOverrides", "an object")),
    };
    for (name, state) in &overrides {
        if !state.as_str().is_some_and(|s| STATES.contains(&s)) {
            return Err(shape(
                &format!("skillOverrides.{name}"),
                "\"on\", \"name-only\", \"user-invocable-only\" or \"off\"",
            ));
        }
    }

    let ours: BTreeSet<String> = recorded(&doc, RECORD)?
        .into_iter()
        .filter(|n| overrides.get(n).and_then(Value::as_str) == Some("off"))
        .collect();
    let had_entries = !overrides.is_empty();
    let mut kept = BTreeSet::new();
    for name in &ours {
        if off.contains(name) {
            kept.insert(name.clone());
        } else {
            overrides.shift_remove(name);
        }
    }
    for name in off {
        if !overrides.contains_key(name) {
            overrides.insert(name.clone(), Value::from("off"));
            kept.insert(name.clone());
        }
    }

    if !overrides.is_empty() {
        doc.insert("skillOverrides".into(), Value::Object(overrides));
    } else if had_entries {
        doc.shift_remove("skillOverrides");
    }
    record(&mut doc, RECORD, &kept);
    finish(&original, &doc)
}
