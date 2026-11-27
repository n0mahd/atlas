use serde::Deserialize;

use crate::kernel::{HarnessKind, Scope, Slug};

/// The moment a hook runs, in Atlas's own words. Each harness adapter maps these to the
/// harness's event names; a harness without the event doesn't get the hook.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    SessionStart,
    UserPrompt,
    PreToolUse,
    PostToolUse,
    Stop,
}

impl HookEvent {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::SessionStart => "session start",
            Self::UserPrompt => "user prompt",
            Self::PreToolUse => "before a tool runs",
            Self::PostToolUse => "after a tool runs",
            Self::Stop => "agent stops",
        }
    }
}

/// `hook.toml`: one hook, defined once for every harness (decision 0011).
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HookManifest {
    pub description: String,
    pub event: HookEvent,
    /// Which tools it applies to, for tool events (`Bash`, `Edit|Write`). None: all.
    pub matcher: Option<String>,
    /// The script to run, a file in the hook's folder.
    pub run: String,
    /// Seconds before the harness gives up on it.
    pub timeout: Option<u32>,
    /// Limit it to these harnesses. None: every harness that has the event.
    pub harnesses: Option<Vec<HarnessKind>>,
}

/// A hook folder with its manifest.
#[derive(Clone, Debug)]
pub struct HookDef {
    pub name: Slug,
    pub scope: Scope,
    pub manifest: HookManifest,
    /// Where `hook.toml` lives, for display.
    pub path: String,
    /// Whether the script named in `run` exists in the folder.
    pub script_present: bool,
}
