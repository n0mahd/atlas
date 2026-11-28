//! Golden, idempotence and removal tests for the hook formats.
//!
//! Fixtures live in `tests/fixtures/<harness>/`. Run with `BLESS=1` to rewrite the
//! `*.after.*` files from the current output, then review the diff.

use std::fs;
use std::path::PathBuf;

use atlas_core::catalog::HookEvent;
use atlas_core::kernel::Slug;
use atlas_harness::hooks::{ManagedHook, antigravity_hooks, claude_settings, codex_hooks};
use atlas_harness::{Change, FormatError};
use serde_json::Value;

type Format = fn(Option<&[u8]>, &[ManagedHook]) -> Result<Change, FormatError>;

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

fn read(path: &str) -> Vec<u8> {
    fs::read(fixture(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn hook(name: &str, event: HookEvent, matcher: Option<&str>, timeout: Option<u32>) -> ManagedHook {
    ManagedHook {
        name: Slug::new(name).unwrap(),
        event,
        matcher: matcher.map(str::to_owned),
        script: format!("/opt/atlas/store/hooks/{name}/run.sh"),
        timeout,
        replaces: None,
    }
}

fn desired() -> Vec<ManagedHook> {
    vec![
        hook(
            "format-on-save",
            HookEvent::PostToolUse,
            Some("Edit|Write"),
            Some(30),
        ),
        hook("guard", HookEvent::PreToolUse, Some("Bash"), Some(20)),
        hook("session-note", HookEvent::SessionStart, None, None),
        hook("prompt-log", HookEvent::UserPrompt, None, Some(5)),
    ]
}

fn written(change: Change) -> Vec<u8> {
    match change {
        Change::Write(bytes) => bytes,
        other => panic!("expected a write, got {other:?}"),
    }
}

/// Applies `format` and checks the output against the golden file.
fn golden(format: Format, before: &str, after: &str, desired: &[ManagedHook]) -> Vec<u8> {
    let out = written(format(Some(&read(before)), desired).unwrap());
    if std::env::var_os("BLESS").is_some() {
        fs::write(fixture(after), &out).unwrap();
    }
    assert_eq!(
        String::from_utf8(out.clone()).unwrap(),
        String::from_utf8(read(after)).unwrap(),
        "{after} differs; rerun with BLESS=1 and review"
    );
    out
}

fn json(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

fn commands(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    collect(v, &mut out);
    out
}

fn collect(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, v) in m {
                if k == "command" {
                    out.push(v.as_str().unwrap().to_owned());
                } else {
                    collect(v, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|v| collect(v, out)),
        _ => {}
    }
}

const CLAUDE_USER_COMMANDS: [&str; 4] = [
    "bash ~/.claude/hooks/block-force-push.sh",
    "python3 ~/.claude/hooks/no-worktrees.py",
    "npx prettier --write \"$CLAUDE_FILE_PATHS\"",
    "echo 'remember the style guide'",
];

#[test]
fn claude_settings_golden() {
    golden(
        claude_settings,
        "claude/settings.before.json",
        "claude/settings.after.json",
        &desired(),
    );
}

#[test]
fn claude_keeps_everything_that_isnt_atlas() {
    let before = json(&read("claude/settings.before.json"));
    let after = json(&read("claude/settings.after.json"));
    for key in [
        "$schema",
        "model",
        "permissions",
        "env",
        "statusLine",
        "theme",
    ] {
        assert_eq!(before[key], after[key], "{key}");
    }
    let keys: Vec<_> = after.as_object().unwrap().keys().collect();
    let before_keys: Vec<_> = before.as_object().unwrap().keys().collect();
    assert_eq!(keys, before_keys, "key order");
    let cmds = commands(&after);
    for user in CLAUDE_USER_COMMANDS {
        assert!(cmds.iter().any(|c| c == user), "{user}");
    }
    assert!(!cmds.iter().any(|c| c.contains("atlas:stray")));
    assert!(!cmds.iter().any(|c| c.contains("atlas:old-reminder")));
    assert!(
        after["hooks"].get("Stop").is_none(),
        "emptied event dropped"
    );
}

#[test]
fn claude_is_idempotent() {
    let after = read("claude/settings.after.json");
    assert_eq!(
        claude_settings(Some(&after), &desired()).unwrap(),
        Change::Unchanged
    );
}

#[test]
fn claude_removes_a_dropped_hook_and_keeps_user_hooks() {
    let after = read("claude/settings.after.json");
    let fewer: Vec<_> = desired()
        .into_iter()
        .filter(|h| h.name.as_str() != "format-on-save")
        .collect();
    let out = json(&written(claude_settings(Some(&after), &fewer).unwrap()));
    let cmds = commands(&out);
    assert!(!cmds.iter().any(|c| c.contains("atlas:format-on-save")));
    assert!(cmds.iter().any(|c| c.contains("atlas:guard")));
    for user in CLAUDE_USER_COMMANDS {
        assert!(cmds.iter().any(|c| c == user), "{user}");
    }
}

#[test]
fn claude_removing_every_atlas_hook_leaves_only_the_users() {
    let after = read("claude/settings.after.json");
    let out = json(&written(claude_settings(Some(&after), &[]).unwrap()));
    let mut cmds = commands(&out);
    cmds.retain(|c| !c.starts_with('~'));
    cmds.sort();
    let mut user = CLAUDE_USER_COMMANDS.map(str::to_owned).to_vec();
    user.sort();
    assert_eq!(cmds, user);
}

#[test]
fn claude_file_with_only_atlas_hooks_is_removed() {
    let only = read("claude/settings.only-atlas.json");
    assert_eq!(claude_settings(Some(&only), &[]).unwrap(), Change::Remove);
}

#[test]
fn claude_missing_file_is_created_then_removed() {
    assert_eq!(claude_settings(None, &[]).unwrap(), Change::Unchanged);
    let created = written(claude_settings(None, &desired()).unwrap());
    assert_eq!(
        claude_settings(Some(&created), &desired()).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        claude_settings(Some(&created), &[]).unwrap(),
        Change::Remove
    );
}

#[test]
fn claude_moves_a_hook_whose_event_changed() {
    let after = read("claude/settings.after.json");
    let mut moved = desired();
    moved[1].event = HookEvent::PostToolUse;
    let out = json(&written(claude_settings(Some(&after), &moved).unwrap()));
    let pre = commands(&out["hooks"]["PreToolUse"]);
    let post = commands(&out["hooks"]["PostToolUse"]);
    assert!(!pre.iter().any(|c| c.contains("atlas:guard")));
    assert!(post.iter().any(|c| c.contains("atlas:guard")));
}

#[test]
fn claude_keeps_a_users_empty_hooks_object() {
    let file = b"{\n  \"hooks\": {}\n}\n";
    assert_eq!(claude_settings(Some(file), &[]).unwrap(), Change::Unchanged);
}

#[test]
fn claude_refuses_files_it_cannot_read() {
    assert!(matches!(
        claude_settings(Some(b"{ \"hooks\": "), &desired()),
        Err(FormatError::InvalidJson(_))
    ));
    assert!(matches!(
        claude_settings(Some(b"{ \"hooks\": [] }"), &desired()),
        Err(FormatError::UnexpectedShape { .. })
    ));
    assert!(matches!(
        claude_settings(Some(b"[]"), &desired()),
        Err(FormatError::UnexpectedShape { .. })
    ));
}

#[test]
fn codex_hooks_golden_and_idempotent() {
    let out = golden(
        codex_hooks,
        "codex/hooks.before.json",
        "codex/hooks.after.json",
        &desired(),
    );
    assert_eq!(
        codex_hooks(Some(&out), &desired()).unwrap(),
        Change::Unchanged
    );
    let cmds = commands(&json(&out));
    assert!(cmds.contains(&"python3 ~/.codex/hooks/pre_tool_use_policy.py".to_owned()));
    assert!(!cmds.iter().any(|c| c.contains("atlas:old-reminder")));
}

#[test]
fn antigravity_hooks_golden_and_idempotent() {
    let out = golden(
        antigravity_hooks,
        "antigravity/hooks.before.json",
        "antigravity/hooks.after.json",
        &desired(),
    );
    assert_eq!(
        antigravity_hooks(Some(&out), &desired()).unwrap(),
        Change::Unchanged
    );
    let v = json(&out);
    let keys: Vec<_> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        [
            "lint-checker",
            "atlas:format-on-save",
            "safety-gate",
            "atlas:guard",
            "atlas:session-note",
        ],
        "user hooks stay in place, updated ones keep their slot, user-prompt is skipped"
    );
    assert_eq!(v["safety-gate"]["enabled"], Value::Bool(false));
}

#[test]
fn antigravity_removes_only_atlas_hooks() {
    let out = read("antigravity/hooks.after.json");
    let v = json(&written(antigravity_hooks(Some(&out), &[]).unwrap()));
    let keys: Vec<_> = v.as_object().unwrap().keys().map(String::as_str).collect();
    assert_eq!(keys, ["lint-checker", "safety-gate"]);
}

/// A hook brought into Atlas from a hand-made entry.
fn imported(command: &str) -> ManagedHook {
    ManagedHook {
        replaces: Some(command.to_owned()),
        ..hook(
            "no-worktrees",
            HookEvent::PreToolUse,
            Some("Bash"),
            Some(10),
        )
    }
}

#[test]
fn an_imported_hook_replaces_only_its_identical_hand_made_entry() {
    let before = br#"{
  "model": "x",
  "hooks": {
    "PreToolUse": [
      {"matcher": "Bash", "hooks": [
        {"type": "command", "command": "python3 ~/.claude/hooks/no-worktrees.py", "timeout": 10},
        {"type": "command", "command": "bash ~/.claude/hooks/other.sh"}
      ]},
      {"matcher": "Edit", "hooks": [
        {"type": "command", "command": "python3 ~/.claude/hooks/no-worktrees.py", "timeout": 10}
      ]},
      {"matcher": "Bash", "hooks": [
        {"type": "command", "command": "python3 ~/.claude/hooks/no-worktrees.py", "timeout": 30}
      ]}
    ]
  }
}"#;
    let hook = imported("python3 ~/.claude/hooks/no-worktrees.py");
    for format in [claude_settings as Format, codex_hooks] {
        let out = written(format(Some(before), std::slice::from_ref(&hook)).unwrap());
        let v = json(&out);
        let groups = v["hooks"]["PreToolUse"].as_array().unwrap();
        // The identical handler goes; its group keeps the owner's other handler.
        assert_eq!(
            commands(&groups[0]),
            ["bash ~/.claude/hooks/other.sh"],
            "{groups:?}"
        );
        // Another matcher or another timeout isn't the same entry, so it stays.
        assert_eq!(groups[1]["matcher"], "Edit");
        assert_eq!(groups[2]["hooks"][0]["timeout"], 30);
        assert!(commands(&groups[3])[0].ends_with("# atlas:no-worktrees"));
        assert_eq!(groups.len(), 4);
        assert_eq!(v["model"], "x");
        assert_eq!(
            format(Some(&out), std::slice::from_ref(&hook)).unwrap(),
            Change::Unchanged
        );
    }
}

#[test]
fn a_group_emptied_by_an_imported_hook_goes() {
    let before = br#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"python3 ~/.claude/hooks/no-worktrees.py","timeout":10}]}]}}"#;
    let brought = imported("python3 ~/.claude/hooks/no-worktrees.py");
    let out = json(&written(claude_settings(Some(before), &[brought]).unwrap()));
    let groups = out["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 1);
    assert!(commands(&groups[0])[0].ends_with("# atlas:no-worktrees"));
    // Without `replaces` the hand-made entry stays beside Atlas's.
    let plain = hook(
        "no-worktrees",
        HookEvent::PreToolUse,
        Some("Bash"),
        Some(10),
    );
    let out = json(&written(claude_settings(Some(before), &[plain]).unwrap()));
    assert_eq!(out["hooks"]["PreToolUse"].as_array().unwrap().len(), 2);
}
