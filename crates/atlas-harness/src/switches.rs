//! Per-agent skill switches: an agent that reads a folder holding a skill it must not use
//! is told so in its own settings, as an entry Atlas manages. Pure formats, like `hooks`:
//! current bytes + the skill names that must be off → new bytes.
//!
//! CONTRACT (decision 0027). The `switches` worker implements the formats and keeps this
//! table true against each harness's docs or source (recorded in `HARNESSES.md`):
//! - Claude Code: `skillOverrides` in `~/.claude/settings.json`; in a repo
//!   `.claude/settings.local.json` (kept out of git).
//! - Codex: `[[skills.config]]` entries with `enabled = false` in `~/.codex/config.toml`.
//! - `OpenCode`: `permission.skill` deny entries in `opencode.json`/`opencode.jsonc`,
//!   preserving comments.
//! - Pi: its settings file.
//!
//! Antigravity, Amp, Muse and Grok Build have no such setting known to Atlas, so their
//! switch is reported instead (`placement::Solved::stuck_on`).

use std::collections::BTreeSet;
use std::path::Path;

use atlas_core::kernel::HarnessKind;

use crate::placement::Reach;
use crate::{Change, FormatError};

/// Where an agent's skill switches live, relative to the home (`Reach::Home`) or the repo.
#[must_use]
pub const fn switch_file(kind: HarnessKind, reach: Reach) -> Option<&'static str> {
    match (kind, reach) {
        (HarnessKind::ClaudeCode, Reach::Home) => Some(".claude/settings.json"),
        (HarnessKind::ClaudeCode, Reach::Project) => Some(".claude/settings.local.json"),
        (HarnessKind::Codex, Reach::Home) => Some(".codex/config.toml"),
        (HarnessKind::OpenCode, Reach::Home) => Some(".config/opencode/opencode.json"),
        (HarnessKind::OpenCode, Reach::Project) => Some("opencode.json"),
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
/// that key switches by path (Codex).
///
/// # Errors
/// The current file can't be parsed; Atlas then leaves it alone and reports it.
pub fn render_switches(
    kind: HarnessKind,
    reach: Reach,
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
    skill_dir: &dyn Fn(&str) -> Option<&Path>,
) -> Result<Change, FormatError> {
    let _ = (kind, reach, current, off, skill_dir);
    // Placeholder until the switches worker lands the formats: change nothing, so a
    // client with this build never writes a half-understood file.
    Ok(Change::Unchanged)
}
