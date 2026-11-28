//! Antigravity's `hooks.json`: a top-level object of named hooks, each mapping events
//! to handlers. Atlas's hooks are the keys `atlas:<name>`.

use atlas_core::catalog::HookEvent;
use serde_json::{Map, Value};

use super::{ManagedHook, handler, shell_quote};
use crate::change::{Change, FormatError};
use crate::json::{finish, parse_object};

/// The prefix of every hook name Atlas writes.
pub const PREFIX: &str = "atlas:";

/// Antigravity's name for an Atlas event. It has no event for a submitted prompt
/// (`PreInvocation` fires before every model call), so `user-prompt` hooks skip it.
#[must_use]
pub const fn antigravity_event(event: HookEvent) -> Option<&'static str> {
    match event {
        HookEvent::SessionStart => Some("SessionStart"),
        HookEvent::UserPrompt => None,
        HookEvent::PreToolUse => Some("PreToolUse"),
        HookEvent::PostToolUse => Some("PostToolUse"),
        HookEvent::Stop => Some("Stop"),
    }
}

/// Tool events wrap handlers in matcher groups; the others list handlers directly.
fn is_tool_event(event: HookEvent) -> bool {
    matches!(event, HookEvent::PreToolUse | HookEvent::PostToolUse)
}

fn spec(hook: &ManagedHook, event: &str) -> Value {
    let h = handler(hook, shell_quote(&hook.script));
    let list = if is_tool_event(hook.event) {
        let mut g = Map::new();
        g.insert(
            "matcher".into(),
            hook.matcher.clone().unwrap_or_else(|| "*".into()).into(),
        );
        g.insert("hooks".into(), Value::Array(vec![h]));
        Value::Object(g)
    } else {
        h
    };
    let mut s = Map::new();
    s.insert(event.into(), Value::Array(vec![list]));
    Value::Object(s)
}

/// `~/.gemini/config/hooks.json` with the given Atlas hooks and nothing else of
/// Atlas's. Hooks whose event Antigravity lacks are left out.
///
/// # Errors
/// [`FormatError`] when the file isn't a JSON object; it is then left alone.
pub fn antigravity_hooks(
    current: Option<&[u8]>,
    desired: &[ManagedHook],
) -> Result<Change, FormatError> {
    let original = parse_object(current)?;
    let wanted: Vec<(String, Value)> = desired
        .iter()
        .filter_map(|h| {
            Some((
                format!("{PREFIX}{}", h.name),
                spec(h, antigravity_event(h.event)?),
            ))
        })
        .collect();

    let mut obj = original.clone();
    obj.retain(|key, _| !key.starts_with(PREFIX) || wanted.iter().any(|(k, _)| k == key));
    for (key, value) in wanted {
        // Replacing an existing key keeps its position.
        obj.insert(key, value);
    }
    finish(&original, &obj)
}
