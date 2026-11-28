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
//!   keyed by the skill's `SKILL.md` path, so the home file also switches off a project's
//!   skills without writing into the repo. Atlas's tables end their header line with the
//!   comment `# atlas:<name>`.
//! - `OpenCode`: `permission.skill.<name> = "deny"` in `opencode.jsonc` (else `opencode.json`)
//!   in the home config folder. Home only: a repo's `opencode.json` is usually tracked,
//!   and a switch is the owner's, not the team's. The owner's comments are kept; Atlas's
//!   entries end with the comment `// atlas:<name>`.
//! - Pi: `-skills/<name>/SKILL.md` in the `skills` list of `~/.pi/agent/settings.json`,
//!   home only (a repo's `.pi/settings.json` is usually committed). Strict JSON, so Atlas
//!   lists its names under `atlas:skills`.
//!
//! Every format keeps what the owner wrote: an entry of the owner's for a skill (on or
//! off) is never changed or removed, and Atlas adds none of its own beside it. Running a
//! format on its own output gives [`Change::Unchanged`].
//!
//! Antigravity, Amp, Muse and Grok Build have no such setting known to Atlas, so their
//! switch is reported instead (`placement::Solved::stuck_on`).
//!
//! Several things share some of these files (Claude Code's `settings.json` also holds
//! Atlas's hooks), so a caller renders every Atlas part onto the same bytes in turn and
//! writes once. Codex's home and project switches share one file: render it once with
//! every name that must be off there, as a second call would drop the first one's
//! entries.

mod claude;
mod codex;
mod jsonc;
mod opencode;
mod pi;

use std::collections::BTreeSet;
use std::path::PathBuf;

use atlas_core::kernel::{HarnessKind, Slug};
use serde_json::{Map, Value};

use crate::change::shape;
use crate::placement::Reach;
use crate::{Change, FormatError};

/// What a switch file's paths are relative to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Base {
    /// The login's home folder.
    Home,
    /// The repo's top folder.
    Repo,
}

/// Where an agent's skill switches for one reach live.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchFile {
    pub base: Base,
    /// Candidate files in the agent's precedence order: the first that exists is the one
    /// Atlas edits.
    pub candidates: &'static [&'static str],
    /// The file Atlas creates when none of `candidates` exists; one of them.
    pub create: &'static str,
}

/// Where `kind` keeps skill switches for skills it loads at `reach`, if anywhere.
#[must_use]
pub const fn switch_file(kind: HarnessKind, reach: Reach) -> Option<SwitchFile> {
    const fn one(base: Base, file: &'static [&'static str; 1]) -> SwitchFile {
        SwitchFile {
            base,
            candidates: file,
            create: file[0],
        }
    }
    let file = match (kind, reach) {
        (HarnessKind::ClaudeCode, Reach::Home) => one(Base::Home, &[".claude/settings.json"]),
        (HarnessKind::ClaudeCode, Reach::Project) => {
            one(Base::Repo, &[".claude/settings.local.json"])
        }
        (HarnessKind::Codex, _) => one(Base::Home, &[".codex/config.toml"]),
        // OpenCode merges `opencode.json` then `opencode.jsonc`, so a `.jsonc` has the last
        // word when both exist. With neither, `.jsonc` is created too: OpenCode rewrites a
        // `.json` with JSON.stringify when it saves settings, which would drop Atlas's
        // comment marks and leave its entries looking like the owner's.
        (HarnessKind::OpenCode, Reach::Home) => SwitchFile {
            base: Base::Home,
            candidates: &[
                ".config/opencode/opencode.jsonc",
                ".config/opencode/opencode.json",
            ],
            create: ".config/opencode/opencode.jsonc",
        },
        (HarnessKind::Pi, Reach::Home) => one(Base::Home, &[".pi/agent/settings.json"]),
        _ => return None,
    };
    Some(file)
}

/// Whether Atlas can switch a skill off for `kind` at this reach.
#[must_use]
pub const fn can_switch_off(kind: HarnessKind, reach: Reach) -> bool {
    switch_file(kind, reach).is_some()
}

/// Rewrites one switch file so exactly `off` (skill names) are switched off by Atlas.
///
/// - Entries the owner wrote are kept, including the owner's own "off" for a skill.
/// - `atlas_before` is the set of names Atlas switched off in this file last time
///   (atlasd keeps it in its state). Every format here marks Atlas's entries in the file
///   itself (a comment, or for strict JSON a list under Atlas's own key), so they don't
///   need it: the file is the record, and an entry the owner had before Atlas wanted the
///   same skill off is never taken for Atlas's.
/// - `skill_dir` maps a name to the folder the agent loads it from, for formats keyed by
///   path (Codex); a link into the skill's folder is fine. At `Reach::Project` a name it
///   doesn't map is skipped, since a name entry would also switch off a global skill.
/// - Only names that are slugs (Atlas's skill names) are written: Atlas's marks hold the
///   name, and a slug can't make a wildcard (`OpenCode`) or end a comment early.
/// - `Change::Remove` only when nothing is left but what Atlas made: an empty object or
///   table, or the file Atlas created with no entries in it.
///
/// # Errors
/// The current file can't be parsed, or holds the setting in a shape Atlas doesn't edit;
/// Atlas then leaves it alone and reports it.
pub fn render_switches(
    kind: HarnessKind,
    reach: Reach,
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
    atlas_before: &BTreeSet<String>,
    skill_dir: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Change, FormatError> {
    let _ = atlas_before;
    if !can_switch_off(kind, reach) {
        return Ok(Change::Unchanged);
    }
    let off: BTreeSet<String> = off
        .iter()
        .filter(|n| Slug::new(n.as_str()).is_ok())
        .cloned()
        .collect();
    let off = &off;
    match kind {
        HarnessKind::ClaudeCode => claude::render(current, off),
        HarnessKind::Codex => codex::render(current, off, reach, skill_dir),
        HarnessKind::OpenCode => opencode::render(current, off),
        HarnessKind::Pi => pi::render(current, off),
        _ => Ok(Change::Unchanged),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_switch_path_stays_inside_and_project_files_are_untracked_by_design() {
        for kind in HarnessKind::ALL {
            for reach in [Reach::Home, Reach::Project] {
                let Some(f) = switch_file(kind, reach) else {
                    continue;
                };
                assert!(f.candidates.contains(&f.create), "{kind}");
                for p in f.candidates {
                    assert!(!p.starts_with('/') && !p.split('/').any(|s| s == ".."));
                    if f.base == Base::Repo {
                        assert!(p.contains(".local."), "{p} must be a local-only file");
                    }
                }
            }
        }
        assert!(can_switch_off(HarnessKind::Codex, Reach::Project));
        assert!(!can_switch_off(HarnessKind::OpenCode, Reach::Project));
    }

    #[test]
    fn opencode_edits_the_file_it_reads_last() {
        // OpenCode merges `opencode.json` then `opencode.jsonc`, so with both present the
        // `.jsonc` wins, and it rewrites a `.json` without comments: Atlas edits and
        // creates the `.jsonc`.
        let f = switch_file(HarnessKind::OpenCode, Reach::Home).unwrap();
        assert_eq!(
            f.candidates,
            [
                ".config/opencode/opencode.jsonc",
                ".config/opencode/opencode.json"
            ]
        );
        assert_eq!(f.create, ".config/opencode/opencode.jsonc");
    }

    #[test]
    fn harness_facts_table_covers_every_switch_file() {
        let doc = include_str!("../HARNESSES.md");
        for kind in HarnessKind::ALL {
            for reach in [Reach::Home, Reach::Project] {
                for p in switch_file(kind, reach).map_or(&[][..], |f| f.candidates) {
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
            (HarnessKind::OpenCode, Reach::Project),
            (HarnessKind::Pi, Reach::Project),
        ] {
            assert!(!can_switch_off(kind, reach));
            assert_eq!(
                render_switches(kind, reach, Some(b"{}"), &off, &off, &|_| None).unwrap(),
                Change::Unchanged
            );
        }
    }
}
