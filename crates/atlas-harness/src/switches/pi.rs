//! Pi: an exact exclusion, `-skills/<name>/SKILL.md`, in the `skills` list of
//! `~/.pi/agent/settings.json`.
//!
//! Pi applies that list to the skills it finds in `~/.pi/agent/skills` and
//! `~/.agents/skills`, matching an exact entry against a skill's `SKILL.md` or folder,
//! relative to either base folder or absolute; `-skills/<name>/SKILL.md` is the form
//! `pi config` writes. Pi reads the file with `JSON.parse`, so there is no room for a
//! comment: Atlas lists its names under its own top-level key, [`RECORD`], which Pi keeps
//! when it saves the file (it rewrites only the fields it changed). A name counts as
//! Atlas's only while it is in that list and its exact entry is still there.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{record, recorded};
use crate::change::shape;
use crate::json::{finish, parse_object};
use crate::{Change, FormatError};

/// Atlas's own key: the names whose exclusion Atlas wrote.
pub(super) const RECORD: &str = "atlas:skills";

fn entry(name: &str) -> String {
    format!("-skills/{name}/SKILL.md")
}

/// Whether an owner's exact entry (`+` or `-`) names the skill. Glob exclusions (`!…`)
/// aren't evaluated.
fn names_skill(entry: &str, name: &str) -> bool {
    let Some(path) = entry.strip_prefix(['-', '+']) else {
        return false;
    };
    let path = path.strip_prefix("./").unwrap_or(path);
    let folder = format!("skills/{name}");
    let file = format!("{folder}/SKILL.md");
    path == folder
        || path == file
        || path.ends_with(&format!("/{folder}"))
        || path.ends_with(&format!("/{file}"))
}

pub(super) fn render(
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
) -> Result<Change, FormatError> {
    let original = parse_object(current)?;
    let mut doc = original.clone();
    let mut skills: Vec<String> = match doc.get("skills") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or_else(|| shape("skills", "a list of paths"))?,
        // Pi's old `{customDirectories: [...]}` shape, which Pi migrates when it loads.
        Some(_) => return Err(shape("skills", "a list of paths")),
    };

    let ours: BTreeSet<String> = recorded(&doc, RECORD)?
        .into_iter()
        .filter(|n| skills.contains(&entry(n)))
        .collect();
    let had_entries = !skills.is_empty();
    let mut kept = BTreeSet::new();
    for name in &ours {
        if off.contains(name) {
            kept.insert(name.clone());
        } else if let Some(at) = skills.iter().rposition(|e| *e == entry(name)) {
            skills.remove(at);
        }
    }
    for name in off {
        if !ours.contains(name) && !skills.iter().any(|e| names_skill(e, name)) {
            skills.push(entry(name));
            kept.insert(name.clone());
        }
    }

    if !skills.is_empty() {
        let list = skills.into_iter().map(Value::String).collect();
        doc.insert("skills".into(), Value::Array(list));
    } else if had_entries {
        doc.shift_remove("skills");
    }
    record(&mut doc, RECORD, &kept);
    finish(&original, &doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_entries_are_recognised_in_every_exact_form() {
        for e in [
            "-skills/pdf",
            "-skills/pdf/SKILL.md",
            "-./skills/pdf/SKILL.md",
            "+skills/pdf",
            "-/home/u/.agents/skills/pdf",
            "-/home/u/.pi/agent/skills/pdf/SKILL.md",
        ] {
            assert!(names_skill(e, "pdf"), "{e}");
        }
        for e in [
            "skills/pdf",
            "!skills/pdf",
            "-skills/pdf-tools",
            "-skills/x/pdf",
        ] {
            assert!(!names_skill(e, "pdf"), "{e}");
        }
    }
}
