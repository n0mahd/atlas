//! Per-agent skill switches: an agent that reads a folder holding a skill it must not use
//! is told so in its own settings, as an entry Atlas manages. Pure formats, like `hooks`:
//! current bytes + the skill names that must be off → new bytes.
//!
//! Decision 0027; the facts and where each comes from are in `HARNESSES.md` ("Skill
//! switches"):
//! - Claude Code: `skillOverrides: {"<name>": "off"}` in `~/.claude/settings.json`; in a
//!   repo `.claude/settings.local.json` (kept out of git). Strict JSON, so Atlas lists the
//!   names it switched off under its own top-level key `atlas:skillOverrides`.
//! - Codex: `[[skills.config]]` tables with `enabled = false` in `~/.codex/config.toml`,
//!   keyed by the skill's `SKILL.md` path (by `name` when no folder is known). Atlas's
//!   tables end their header line with the comment `# atlas:<name>`.
//! - `OpenCode`: `permission.skill.<name> = "deny"` in `opencode.jsonc` (the home config
//!   folder, or the repo root). The owner's comments are kept; Atlas's entries end with
//!   the comment `// atlas:<name>`.
//! - Pi: `-skills/<name>/SKILL.md` in the `skills` list of `~/.pi/agent/settings.json`
//!   (home only: a repo's `.pi/settings.json` is usually committed). Strict JSON, so
//!   Atlas lists its names under `atlas:skills`.
//!
//! Every format keeps what the owner wrote: an entry of the owner's for a skill (on or
//! off) is never changed or removed, and Atlas adds none of its own beside it. Running a
//! format on its own output gives [`Change::Unchanged`].
//!
//! Antigravity, Amp, Muse and Grok Build have no such setting known to Atlas, so their
//! switch is reported instead (`placement::Solved::stuck_on`).

mod claude;
mod codex;
mod jsonc;
mod opencode;
mod pi;

use std::collections::BTreeSet;
use std::path::PathBuf;

use atlas_core::kernel::HarnessKind;
use serde_json::{Map, Value};

use crate::placement::Reach;
use crate::{Change, FormatError};

/// Where an agent's skill switches live, relative to the home (`Reach::Home`) or the repo.
#[must_use]
pub const fn switch_file(kind: HarnessKind, reach: Reach) -> Option<&'static str> {
    match (kind, reach) {
        (HarnessKind::ClaudeCode, Reach::Home) => Some(".claude/settings.json"),
        (HarnessKind::ClaudeCode, Reach::Project) => Some(".claude/settings.local.json"),
        (HarnessKind::Codex, Reach::Home) => Some(".codex/config.toml"),
        (HarnessKind::OpenCode, Reach::Home) => Some(".config/opencode/opencode.jsonc"),
        (HarnessKind::OpenCode, Reach::Project) => Some("opencode.jsonc"),
        (HarnessKind::Pi, Reach::Home) => Some(".pi/agent/settings.json"),
        _ => None,
    }
}

/// Whether Atlas can switch a skill off for `kind` at this reach.
#[must_use]
pub const fn can_switch_off(kind: HarnessKind, reach: Reach) -> bool {
    switch_file(kind, reach).is_some()
}

/// Rewrites `kind`'s switch file so exactly `off` (skill names) are switched off by
/// Atlas. Entries Atlas didn't write are kept; Atlas's entries for other names are
/// removed. `skill_dir` maps a name to the folder the agent loads it from, for formats
/// that key switches by path (Codex); a link into the skill's folder is fine.
///
/// # Errors
/// The current file can't be parsed, or holds the setting in a shape Atlas doesn't edit;
/// Atlas then leaves it alone and reports it.
pub fn render_switches(
    kind: HarnessKind,
    reach: Reach,
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
    skill_dir: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Change, FormatError> {
    if !can_switch_off(kind, reach) {
        return Ok(Change::Unchanged);
    }
    match kind {
        HarnessKind::ClaudeCode => claude::render(current, off),
        HarnessKind::Codex => codex::render(current, off, skill_dir),
        HarnessKind::OpenCode => opencode::render(current, off),
        HarnessKind::Pi => pi::render(current, off),
        _ => Ok(Change::Unchanged),
    }
}

/// Parses a strict JSON object file. A missing or blank file is an empty object.
fn parse_object(current: Option<&[u8]>) -> Result<Map<String, Value>, FormatError> {
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

fn shape(at: &str, expected: &'static str) -> FormatError {
    FormatError::UnexpectedShape {
        at: at.to_owned(),
        expected,
    }
}

/// The names Atlas recorded under `key` (its own top-level list in a JSON file).
fn recorded(doc: &Map<String, Value>, key: &str) -> Result<BTreeSet<String>, FormatError> {
    match doc.get(key) {
        None => Ok(BTreeSet::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|v| v.as_str().map(str::to_owned))
            .collect::<Option<_>>()
            .ok_or_else(|| shape(key, "a list of skill names")),
        Some(_) => Err(shape(key, "a list of skill names")),
    }
}

/// Stores Atlas's list under `key`, or drops the key when the list is empty.
fn record(doc: &mut Map<String, Value>, key: &str, names: &BTreeSet<String>) {
    if names.is_empty() {
        doc.shift_remove(key);
    } else {
        let list = names.iter().cloned().map(Value::String).collect();
        doc.insert(key.to_owned(), Value::Array(list));
    }
}

/// Renders an edited JSON object the way the hook formats do: two-space JSON with a
/// trailing newline; an object left empty means the file goes.
fn finish(
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
    fn harness_facts_table_covers_every_switch_file() {
        let doc = include_str!("../HARNESSES.md");
        for kind in HarnessKind::ALL {
            for reach in [Reach::Home, Reach::Project] {
                if let Some(p) = switch_file(kind, reach) {
                    assert!(doc.contains(&format!("`{p}`")), "HARNESSES.md lacks {p}");
                }
            }
        }
    }

    #[test]
    fn agents_without_a_setting_are_left_alone() {
        let off = BTreeSet::from(["pdf".to_owned()]);
        for (kind, reach) in [
            (HarnessKind::Grok, Reach::Home),
            (HarnessKind::Antigravity, Reach::Home),
            (HarnessKind::Codex, Reach::Project),
            (HarnessKind::Pi, Reach::Project),
        ] {
            assert!(!can_switch_off(kind, reach));
            assert_eq!(
                render_switches(kind, reach, Some(b"{}"), &off, &|_| None).unwrap(),
                Change::Unchanged
            );
        }
    }
}
