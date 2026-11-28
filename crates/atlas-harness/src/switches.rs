//! Per-agent skill switches: an agent that reads a folder holding a skill it must not use
//! is told so in its own settings, as an entry Atlas manages. Pure formats, like `hooks`:
//! current bytes + the skill names that must be off → new bytes.
//!
//! CONTRACT (decision 0027). The `switches` worker implements the formats and keeps this
//! table true against each harness's docs or source (recorded in `HARNESSES.md`):
//! - Claude Code: `skillOverrides` in `~/.claude/settings.json`; in a repo
//!   `.claude/settings.local.json` (kept out of git).
//! - Codex: `[[skills.config]]` entries with `enabled = false` in `~/.codex/config.toml`,
//!   keyed by the skill folder's absolute path, so the home file also switches off a
//!   project's skills without writing into the repo.
//! - `OpenCode`: `permission.skill` deny entries in `opencode.jsonc` or `opencode.json`
//!   (whichever exists, `.json` when neither), comments kept. Home only: a repo's
//!   `opencode.json` is usually tracked, and a switch is the owner's, not the team's.
//! - Pi: its settings file, home only until a per-repo file is confirmed.
//!
//! Antigravity, Amp, Muse and Grok Build have no such setting known to Atlas, so their
//! switch is reported instead (`placement::Solved::stuck_on`).
//!
//! Several things share some of these files (Claude Code's `settings.json` also holds
//! Atlas's hooks), so a caller renders every Atlas part onto the same bytes in turn and
//! writes once.

use std::collections::BTreeSet;
use std::path::Path;

use atlas_core::kernel::HarnessKind;

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
    /// Candidate files, in preference order: the first that exists is edited, and the
    /// last is created when none does.
    pub candidates: &'static [&'static str],
}

/// Where `kind` keeps skill switches for skills it loads at `reach`, if anywhere.
#[must_use]
pub const fn switch_file(kind: HarnessKind, reach: Reach) -> Option<SwitchFile> {
    const fn home(candidates: &'static [&'static str]) -> SwitchFile {
        SwitchFile {
            base: Base::Home,
            candidates,
        }
    }
    let file = match (kind, reach) {
        (HarnessKind::ClaudeCode, Reach::Home) => home(&[".claude/settings.json"]),
        (HarnessKind::ClaudeCode, Reach::Project) => SwitchFile {
            base: Base::Repo,
            candidates: &[".claude/settings.local.json"],
        },
        (HarnessKind::Codex, _) => home(&[".codex/config.toml"]),
        (HarnessKind::OpenCode, Reach::Home) => home(&[
            ".config/opencode/opencode.jsonc",
            ".config/opencode/opencode.json",
        ]),
        (HarnessKind::Pi, Reach::Home) => home(&[".pi/agent/settings.json"]),
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
///   (atlasd keeps it in its state). Formats with room for a marker recognise Atlas's
///   entries by it and may ignore this; formats without one remove an entry only when its
///   name is in `atlas_before` and not in `off`.
/// - `skill_dir` maps a name to the folder the agent loads it from, for formats keyed by
///   path (Codex).
/// - `Change::Remove` only when Atlas created the file and nothing else is left in it.
///
/// # Errors
/// The current file can't be parsed; Atlas then leaves it alone and reports it.
pub fn render_switches(
    kind: HarnessKind,
    reach: Reach,
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
    atlas_before: &BTreeSet<String>,
    skill_dir: &dyn Fn(&str) -> Option<&Path>,
) -> Result<Change, FormatError> {
    let _ = (kind, reach, current, off, atlas_before, skill_dir);
    // Placeholder until the switches worker lands the formats: change nothing, so a
    // client with this build never writes a half-understood file.
    Ok(Change::Unchanged)
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
                assert_ne!(f.candidates.len(), 0);
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
}
