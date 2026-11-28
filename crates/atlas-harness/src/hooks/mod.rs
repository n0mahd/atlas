//! Hook configs: Atlas's managed hooks rendered into each harness's own hook file.
//!
//! Every function here is pure: current bytes plus the desired hooks give a
//! [`Change`]. Only entries Atlas wrote are touched; everything else in the file keeps
//! its place and value. Running a function on its own output gives
//! [`Change::Unchanged`].
//!
//! - Claude Code (`~/.claude/settings.json`) and Codex (`~/.codex/hooks.json`) share
//!   one shape: `hooks → event → [matcher group] → hooks → [handler]`. Atlas's
//!   handlers are recognised by a trailing shell comment on the command,
//!   `# atlas:<name>` (see [`command_for`]). Each Atlas hook gets a matcher group of
//!   its own, so a user's groups are never edited, only stripped of Atlas handlers.
//! - Antigravity (`~/.gemini/config/hooks.json`) names each hook at the top level,
//!   so Atlas's are the keys starting with `atlas:`.

mod antigravity;
mod grouped;

use atlas_core::catalog::HookEvent;
use atlas_core::kernel::Slug;
use serde_json::{Map, Value};

use crate::change::{Change, FormatError};

pub use antigravity::{antigravity_event, antigravity_hooks};
pub use grouped::{claude_event, claude_settings, codex_event, codex_hooks};

/// One Atlas hook, resolved for a machine: the script's absolute path is known.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagedHook {
    pub name: Slug,
    pub event: HookEvent,
    /// Which tools it applies to, for tool events. None: all.
    pub matcher: Option<String>,
    /// Absolute path of the hook's script on this machine.
    pub script: String,
    /// Seconds before the harness gives up on it.
    pub timeout: Option<u32>,
}

/// What follows the command in a handler Atlas wrote: a shell comment, so the harness
/// runs the command unchanged.
pub const MARKER: &str = " # atlas:";

/// The command Atlas writes for a hook: the quoted script, then `# atlas:<name>`.
///
/// Claude Code and Codex run a handler's `command` through `sh -c` when it has no
/// `args`, so everything from ` #` on is a comment the shell ignores.
#[must_use]
pub fn command_for(hook: &ManagedHook) -> String {
    format!("{}{MARKER}{}", shell_quote(&hook.script), hook.name)
}

/// The Atlas hook name in a command Atlas wrote, if it is one.
#[must_use]
pub fn managed_name(command: &str) -> Option<&str> {
    let (_, name) = command.rsplit_once(MARKER)?;
    Slug::new(name).is_ok().then_some(name)
}

fn shell_quote(s: &str) -> String {
    let plain = !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/._-+=:,@".contains(&b));
    if plain {
        s.to_owned()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

fn handler(hook: &ManagedHook, command: String) -> Value {
    let mut h = Map::new();
    h.insert("type".into(), "command".into());
    h.insert("command".into(), command.into());
    if let Some(t) = hook.timeout {
        h.insert("timeout".into(), t.into());
    }
    Value::Object(h)
}

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

fn shape(at: &str, expected: &'static str) -> FormatError {
    FormatError::UnexpectedShape {
        at: at.to_owned(),
        expected,
    }
}

/// Compares the edited object with the original and renders the outcome: two-space
/// JSON with a trailing newline, the layout the harnesses write themselves.
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

    fn hook(script: &str) -> ManagedHook {
        ManagedHook {
            name: Slug::new("guard").unwrap(),
            event: HookEvent::PreToolUse,
            matcher: None,
            script: script.into(),
            timeout: None,
        }
    }

    #[test]
    fn marker_round_trips() {
        let cmd = command_for(&hook("/opt/atlas/hooks/guard/run.sh"));
        assert_eq!(cmd, "/opt/atlas/hooks/guard/run.sh # atlas:guard");
        assert_eq!(managed_name(&cmd), Some("guard"));
    }

    #[test]
    fn odd_paths_are_quoted() {
        let cmd = command_for(&hook("/opt/my hooks/it's.sh"));
        assert_eq!(cmd, r"'/opt/my hooks/it'\''s.sh' # atlas:guard");
        assert_eq!(managed_name(&cmd), Some("guard"));
    }

    #[test]
    fn user_commands_are_not_managed() {
        for cmd in [
            "bash ~/.claude/hooks/guard.sh",
            "echo hi # atlas:",
            "echo hi # atlas:Not A Slug",
            "echo hi #atlas:guard",
        ] {
            assert_eq!(managed_name(cmd), None, "{cmd}");
        }
    }
}
