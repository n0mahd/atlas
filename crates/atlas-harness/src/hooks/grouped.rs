//! The `hooks → event → [matcher group]` shape Claude Code and Codex share.

use std::collections::HashSet;

use atlas_core::catalog::HookEvent;
use serde_json::{Map, Value};

use super::{ManagedHook, command_for, handler, managed_name};
use crate::change::{Change, FormatError, shape};
use crate::json::{finish, parse_object};

/// Claude Code's name for an Atlas event.
#[must_use]
pub const fn claude_event(event: HookEvent) -> Option<&'static str> {
    Some(match event {
        HookEvent::SessionStart => "SessionStart",
        HookEvent::UserPrompt => "UserPromptSubmit",
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::PostToolUse => "PostToolUse",
        HookEvent::Stop => "Stop",
    })
}

/// Codex's name for an Atlas event. Codex uses Claude Code's names.
#[must_use]
pub const fn codex_event(event: HookEvent) -> Option<&'static str> {
    claude_event(event)
}

/// `~/.claude/settings.json` (or a project's `.claude/settings.json`) with the given
/// Atlas hooks and nothing else of Atlas's.
///
/// # Errors
/// [`FormatError`] when the file isn't JSON or `hooks` isn't shaped as Claude Code
/// expects; the file is then left alone.
pub fn claude_settings(
    current: Option<&[u8]>,
    desired: &[ManagedHook],
) -> Result<Change, FormatError> {
    render(current, desired, claude_event)
}

/// `~/.codex/hooks.json` (or a project's `.codex/hooks.json`) with the given Atlas
/// hooks and nothing else of Atlas's.
///
/// # Errors
/// As [`claude_settings`].
pub fn codex_hooks(current: Option<&[u8]>, desired: &[ManagedHook]) -> Result<Change, FormatError> {
    render(current, desired, codex_event)
}

struct Wanted<'a> {
    event: &'static str,
    name: &'a str,
    group: Value,
    hook: &'a ManagedHook,
}

impl Wanted<'_> {
    /// Whether a hand-made handler in a group with `matcher` under this event is the very
    /// entry this hook was brought in from: same command, matcher and timeout.
    fn replaces(&self, matcher: Option<&str>, handler: &Value) -> bool {
        let Some(command) = self.hook.replaces.as_deref() else {
            return false;
        };
        let kind_ok = handler
            .get("type")
            .is_none_or(|t| t.as_str() == Some("command"));
        kind_ok
            && handler.get("command").and_then(Value::as_str) == Some(command)
            && handler.get("timeout").and_then(Value::as_u64) == self.hook.timeout.map(u64::from)
            && matcher == self.hook.matcher.as_deref()
    }
}

fn group(hook: &ManagedHook) -> Value {
    let mut g = Map::new();
    if let Some(m) = &hook.matcher {
        g.insert("matcher".into(), m.clone().into());
    }
    g.insert(
        "hooks".into(),
        Value::Array(vec![handler(hook, command_for(hook))]),
    );
    Value::Object(g)
}

fn handler_name(h: &Value) -> Option<&str> {
    h.get("command")
        .and_then(Value::as_str)
        .and_then(managed_name)
}

/// The Atlas hook a group belongs to, when it holds exactly one handler and that
/// handler is Atlas's.
fn sole_managed(group: &Value) -> Option<&str> {
    match group.get("hooks")?.as_array()?.as_slice() {
        [only] => handler_name(only),
        _ => None,
    }
}

fn render(
    current: Option<&[u8]>,
    desired: &[ManagedHook],
    event_name: fn(HookEvent) -> Option<&'static str>,
) -> Result<Change, FormatError> {
    let original = parse_object(current)?;
    let wanted: Vec<Wanted> = desired
        .iter()
        .filter_map(|h| {
            Some(Wanted {
                event: event_name(h.event)?,
                name: h.name.as_str(),
                group: group(h),
                hook: h,
            })
        })
        .collect();

    let mut obj = original.clone();
    let had_key = obj.contains_key("hooks");
    let had_entries = obj
        .get("hooks")
        .and_then(Value::as_object)
        .is_some_and(|m| !m.is_empty());
    let hooks = obj
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| shape("hooks", "an object"))?;

    let mut placed: HashSet<(&str, &str)> = HashSet::new();
    let mut emptied = Vec::new();
    for (event, groups) in hooks.iter_mut() {
        let Some(groups) = groups.as_array_mut() else {
            continue;
        };
        let had_groups = !groups.is_empty();
        let mut kept = Vec::with_capacity(groups.len());
        for mut g in groups.drain(..) {
            if let Some(w) = sole_managed(&g).and_then(|name| {
                wanted
                    .iter()
                    .find(|w| w.event == event.as_str() && w.name == name)
                    .filter(|w| !placed.contains(&(w.event, w.name)))
            }) {
                placed.insert((w.event, w.name));
                kept.push(w.group.clone());
                continue;
            }
            let matcher = g
                .get("matcher")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(str::to_owned);
            let replaced = |h: &Value| {
                wanted
                    .iter()
                    .any(|w| w.event == event.as_str() && w.replaces(matcher.as_deref(), h))
            };
            if let Some(handlers) = g.get_mut("hooks").and_then(Value::as_array_mut) {
                let before = handlers.len();
                handlers.retain(|h| handler_name(h).is_none() && !replaced(h));
                if before > 0 && handlers.is_empty() {
                    continue;
                }
            }
            kept.push(g);
        }
        *groups = kept;
        if had_groups && groups.is_empty() {
            emptied.push(event.clone());
        }
    }
    for event in emptied {
        hooks.shift_remove(&event);
    }

    for w in &wanted {
        if placed.insert((w.event, w.name)) {
            hooks
                .entry(w.event)
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .ok_or_else(|| shape(&format!("hooks.{}", w.event), "an array"))?
                .push(w.group.clone());
        }
    }

    // Drop `hooks` once Atlas has emptied it, but keep a user's own empty `{}`.
    if hooks.is_empty() && (had_entries || !had_key) {
        obj.shift_remove("hooks");
    }
    finish(&original, &obj)
}
