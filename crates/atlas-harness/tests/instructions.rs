//! "Use Global instead" and the other instruction rules: the containment check that lets
//! atlasd swap an agent's own file for the link or import, the merge format, Claude
//! Code's AGENTS.md setting and Atlas's block in `.git/info/exclude`.

use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::catalog::ClaudeAgentsMd;
use atlas_harness::Change;
use atlas_harness::hooks::{ManagedHook, claude_settings};
use atlas_harness::instructions::{
    InstructionAction, InstructionConflict, InstructionFile, append_section,
    claude_agents_md_setting, contained, exclude_with, plan_claude_md, plan_instruction_link,
    read_claude_agents_md, without_imports_of,
};

const STORE: &str = "/srv/knowledge/AGENTS.md";

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

fn codex(current: &str, global: &str) -> InstructionAction {
    plan_instruction_link(
        Path::new("/u/.codex/AGENTS.md"),
        &InstructionFile::File(current.to_owned()),
        Path::new(STORE),
        global,
        &[p("/srv/knowledge")],
    )
}

fn claude(current: &str, global: &str) -> InstructionAction {
    plan_claude_md(
        Path::new("/u/.claude/CLAUDE.md"),
        &InstructionFile::File(current.to_owned()),
        Path::new(STORE),
        global,
        &[p("/srv/knowledge")],
        Path::new("/u"),
    )
}

#[test]
fn containment_is_exact_per_non_blank_line() {
    let global = "# Global\n\nBe brief.\n- Use tabs\n";
    assert!(contained("Be brief.\n\n\n- Use tabs\n", global, |_| false));
    assert!(
        contained("Be brief.   \r\n", global, |_| false),
        "line endings and trailing spaces"
    );
    assert!(
        contained("", global, |_| false),
        "an empty file says nothing"
    );
    assert!(
        !contained("Be brief\n", global, |_| false),
        "a missing full stop counts"
    );
    assert!(
        !contained("  Be brief.\n", global, |_| false),
        "indentation counts"
    );
    assert!(!contained("Be brief. Always.\n", global, |_| false));
}

#[test]
fn an_own_file_fully_in_global_becomes_the_link() {
    let global = "# Global\n\n<!-- atlas: from ~/.codex/AGENTS.md -->\n\nUse rg.\nNo emojis.\n";
    assert_eq!(
        codex("Use rg.\n\nNo emojis.\n", global),
        InstructionAction::ReplaceFileWithLink { target: p(STORE) }
    );
    assert_eq!(
        codex("Use rg.\nNo emojis.\nAnd one more.\n", global),
        InstructionAction::Conflict(InstructionConflict::UserFile),
        "a line Global lacks keeps the file"
    );
}

#[test]
fn claudes_own_text_in_global_leaves_just_the_import() {
    let global = "# Global\nUse rg.\n";
    // Its own text, with or without the import already there.
    for text in [
        "Use rg.\n",
        "Use rg.\n@/srv/knowledge/AGENTS.md\n",
        "@~/Knowledge/AGENTS.md\nUse rg.\n",
    ] {
        let plan = plan_claude_md(
            Path::new("/u/.claude/CLAUDE.md"),
            &InstructionFile::File(text.to_owned()),
            Path::new(STORE),
            global,
            &[p("/srv/knowledge"), p("/u/Knowledge")],
            Path::new("/u"),
        );
        assert_eq!(
            plan,
            InstructionAction::Write(format!("@{STORE}\n").into_bytes()),
            "{text:?}"
        );
    }
    // Text Global lacks: the import is added and everything stays.
    assert_eq!(
        claude("Use rg.\nMine only.\n", global),
        InstructionAction::Write(format!("Use rg.\nMine only.\n@{STORE}\n").into_bytes())
    );
    // Just the import: nothing to do.
    assert_eq!(
        claude(&format!("@{STORE}\n"), global),
        InstructionAction::Keep
    );
}

#[test]
fn merging_appends_a_marked_section_once() {
    let into = "# Shop\n\nShip on Fridays.\n";
    let out = append_section(into, "CLAUDE.md", "\n\nRun the tests.\n  - twice\n\n");
    assert_eq!(
        out,
        "# Shop\n\nShip on Fridays.\n\n<!-- atlas: from CLAUDE.md -->\n\nRun the tests.\n  - twice\n"
    );
    assert_eq!(
        append_section(&out, "CLAUDE.md", "Run the tests.\n"),
        out,
        "no repeats"
    );
    assert_eq!(
        append_section(into, "x", "\n \n"),
        into,
        "blank text adds nothing"
    );
    assert_eq!(
        append_section("", "~/.codex/AGENTS.md", "Use rg."),
        "<!-- atlas: from ~/.codex/AGENTS.md -->\n\nUse rg.\n"
    );
    assert_eq!(
        append_section("no newline", "a-->b", "x"),
        "no newline\n\n<!-- atlas: from a->b -->\n\nx\n",
        "the marker can't close its comment early"
    );
    // What was appended is now contained, so atlasd may link the source.
    assert!(contained("Run the tests.\n  - twice\n", &out, |_| false));
}

#[test]
fn import_lines_of_agents_md_are_dropped_from_a_merge() {
    let text = "@AGENTS.md\n# Mine\n@~/notes.md\n@./AGENTS.md\n";
    assert_eq!(
        without_imports_of(
            text,
            Path::new("/r/AGENTS.md"),
            Path::new("/r"),
            Path::new("/u")
        ),
        "# Mine\n@~/notes.md\n"
    );
}

#[test]
fn claude_setting_golden_keeps_everything_else() {
    let before = fs::read(fixture("claude/settings.before.json")).unwrap();
    let Change::Write(out) =
        claude_agents_md_setting(Some(&before), ClaudeAgentsMd::Alongside).unwrap()
    else {
        panic!("expected a write")
    };
    if std::env::var_os("BLESS").is_some() {
        fs::write(fixture("claude/settings.agents-md.json"), &out).unwrap();
    }
    assert_eq!(
        String::from_utf8(out.clone()).unwrap(),
        fs::read_to_string(fixture("claude/settings.agents-md.json")).unwrap()
    );
    assert_eq!(
        read_claude_agents_md(Some(&out)).as_deref(),
        Some("claude-md-and-agents-md")
    );
    assert_eq!(
        claude_agents_md_setting(Some(&out), ClaudeAgentsMd::Alongside).unwrap(),
        Change::Unchanged,
        "idempotent"
    );
    let before: serde_json::Value = serde_json::from_slice(&before).unwrap();
    let mut after: serde_json::Value = serde_json::from_slice(&out).unwrap();
    after.as_object_mut().unwrap().remove("pluginConfigs");
    assert_eq!(after, before, "only pluginConfigs changed");
}

#[test]
fn claude_setting_composes_with_the_hooks_format() {
    let hook = ManagedHook {
        name: atlas_core::kernel::Slug::new("guard").unwrap(),
        event: atlas_core::catalog::HookEvent::PreToolUse,
        matcher: Some("Bash".into()),
        script: "/srv/knowledge/hooks/guard/run.sh".into(),
        timeout: None,
        replaces: None,
    };
    let Change::Write(hooks) = claude_settings(None, std::slice::from_ref(&hook)).unwrap() else {
        panic!()
    };
    let Change::Write(both) =
        claude_agents_md_setting(Some(&hooks), ClaudeAgentsMd::Never).unwrap()
    else {
        panic!()
    };
    // Rendering the hooks again over the combined bytes changes nothing, and the other way round.
    assert_eq!(
        claude_settings(Some(&both), &[hook]).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        claude_agents_md_setting(Some(&both), ClaudeAgentsMd::Never).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        read_claude_agents_md(Some(&both)).as_deref(),
        Some("claude-md")
    );
}

#[test]
fn claude_setting_keeps_a_legacy_entry_in_step_and_refuses_odd_shapes() {
    let legacy = br#"{"pluginConfigs":{"agents-md@builtin":{"options":{"instructionFiles":"claude-md"},"enabled":true}}}"#;
    let Change::Write(out) =
        claude_agents_md_setting(Some(legacy), ClaudeAgentsMd::WhenNoClaudeMd).unwrap()
    else {
        panic!()
    };
    let v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    for id in ["cc-plugin-agents-md@builtin", "agents-md@builtin"] {
        assert_eq!(
            v["pluginConfigs"][id]["options"]["instructionFiles"], "claude-md-or-agents-md",
            "{id}"
        );
    }
    assert_eq!(v["pluginConfigs"]["agents-md@builtin"]["enabled"], true);
    assert!(
        claude_agents_md_setting(Some(br#"{"pluginConfigs":[]}"#), ClaudeAgentsMd::Never).is_err()
    );
    assert!(claude_agents_md_setting(Some(b"not json"), ClaudeAgentsMd::Never).is_err());
    assert_eq!(read_claude_agents_md(Some(b"{}")), None);
}

#[test]
fn exclude_block_is_shared_and_leaves_other_lines() {
    let own = "# git ls-files --others --exclude-from=.git/info/exclude\n*.swp\n";
    let once = exclude_with(own, &["/CLAUDE.local.md"]);
    assert_eq!(
        once,
        "# git ls-files --others --exclude-from=.git/info/exclude\n*.swp\n# atlas:begin\n/CLAUDE.local.md\n# atlas:end\n"
    );
    assert_eq!(
        exclude_with(&once, &["/CLAUDE.local.md"]),
        once,
        "idempotent"
    );
    let with_skill = format!("{own}# atlas:begin\n/.claude/skills/x\n# atlas:end\nlast\n");
    assert_eq!(
        exclude_with(&with_skill, &["/CLAUDE.local.md"]),
        format!("{own}# atlas:begin\n/.claude/skills/x\n/CLAUDE.local.md\n# atlas:end\nlast\n"),
        "added to the existing block; lines after it stay"
    );
    assert_eq!(
        exclude_with("", &["/a"]),
        "# atlas:begin\n/a\n# atlas:end\n"
    );
}

#[test]
fn an_unclosed_exclude_block_ends_at_the_end_of_the_file() {
    // A hand-edited file lost Atlas's closing line: the block runs to the end and is
    // closed there, never opened a second time.
    let unclosed = "*.swp\n# atlas:begin\n/a\nmine\n";
    let fixed = exclude_with(unclosed, &["/b"]);
    assert_eq!(fixed, "*.swp\n# atlas:begin\n/a\nmine\n/b\n# atlas:end\n");
    assert_eq!(fixed.matches("# atlas:begin").count(), 1);
    // A later edit finds one closed block, so the owner's lines stay where they are.
    let owner_after = format!("{fixed}*.log\n");
    assert_eq!(
        exclude_with(&owner_after, &["/c"]),
        "*.swp\n# atlas:begin\n/a\nmine\n/b\n/c\n# atlas:end\n*.log\n"
    );
    assert_eq!(
        exclude_with(unclosed, &["/a"]),
        "*.swp\n# atlas:begin\n/a\nmine\n# atlas:end\n"
    );
}

#[test]
fn nothing_to_exclude_and_no_block_leaves_the_file_alone() {
    for text in ["", "*.swp\n", "*.swp", "# just a comment"] {
        assert_eq!(exclude_with(text, &[]), text);
    }
}
