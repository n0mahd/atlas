//! Golden, round-trip and refusal tests for the per-agent skill switches.
//!
//! Fixtures live in `tests/fixtures/switches/<agent>/`: a `*.before.*` file shaped like a
//! real owner's config (their own switches, comments and unrelated settings) and the
//! `*.after.*` file Atlas makes of it. Run with `BLESS=1` to rewrite the `*.after.*`
//! files from the current output, then review the diff.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use atlas_core::kernel::HarnessKind::{self, ClaudeCode, Codex, OpenCode, Pi};
use atlas_harness::placement::Reach::{self, Home, Project};
use atlas_harness::switches::{SwitchOff, render_switches};
use atlas_harness::{Change, FormatError};

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/switches")
        .join(path)
}

fn read(path: &str) -> Vec<u8> {
    fs::read(fixture(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Skills by name, each loaded from the folder it is linked into for Codex (the other
/// formats go by name alone).
fn skills(list: &[&str]) -> BTreeSet<SwitchOff> {
    list.iter()
        .map(|n| SwitchOff::at(*n, format!("/home/u/.agents/skills/{n}/SKILL.md")))
        .collect()
}

fn by_name(list: &[&str]) -> BTreeSet<SwitchOff> {
    list.iter().map(|n| SwitchOff::named(*n)).collect()
}

fn render(
    kind: HarnessKind,
    reach: Reach,
    current: Option<&[u8]>,
    off: &[&str],
) -> Result<Change, FormatError> {
    render_switches(kind, reach, current, &skills(off))
}

fn written(change: Change) -> Vec<u8> {
    match change {
        Change::Write(bytes) => bytes,
        other => panic!("expected a write, got {other:?}"),
    }
}

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).unwrap()
}

/// The owner already has entries for `deploy` (off) and `legacy-context` (on): Atlas
/// adds only the two it needs and leaves those as they are.
const OFF: [&str; 4] = ["deploy", "legacy-context", "pdf-tools", "release-notes"];

struct Case {
    kind: HarnessKind,
    before: &'static str,
    after: &'static str,
}

const CASES: [Case; 4] = [
    Case {
        kind: ClaudeCode,
        before: "claude/settings.before.json",
        after: "claude/settings.after.json",
    },
    Case {
        kind: Codex,
        before: "codex/config.before.toml",
        after: "codex/config.after.toml",
    },
    Case {
        kind: OpenCode,
        before: "opencode/opencode.before.jsonc",
        after: "opencode/opencode.after.jsonc",
    },
    Case {
        kind: Pi,
        before: "pi/settings.before.json",
        after: "pi/settings.after.json",
    },
];

#[test]
fn adding_matches_the_golden_files() {
    for case in CASES {
        let got = written(render(case.kind, Home, Some(&read(case.before)), &OFF).unwrap());
        if std::env::var_os("BLESS").is_some() {
            fs::write(fixture(case.after), &got).unwrap();
        }
        assert_eq!(text(&got), text(&read(case.after)), "{}", case.after);
    }
}

#[test]
fn rendering_its_own_output_changes_nothing() {
    for case in CASES {
        let after = read(case.after);
        assert_eq!(
            render(case.kind, Home, Some(&after), &OFF).unwrap(),
            Change::Unchanged,
            "{}",
            case.after
        );
    }
}

#[test]
fn switching_back_on_restores_the_owners_file() {
    for case in CASES {
        let after = read(case.after);
        let restored = written(render(case.kind, Home, Some(&after), &[]).unwrap());
        assert_eq!(text(&restored), text(&read(case.before)), "{}", case.before);
    }
}

#[test]
fn turning_one_back_on_removes_only_its_entry() {
    for case in CASES {
        let after = read(case.after);
        let one = written(render(case.kind, Home, Some(&after), &["release-notes"]).unwrap());
        let s = text(&one);
        assert!(!s.contains("pdf-tools"), "{}:\n{s}", case.kind);
        assert!(s.contains("release-notes"), "{}:\n{s}", case.kind);
        // The owner's own entries are still there.
        assert!(s.contains("deploy") && s.contains("legacy-context"), "{s}");
        // And going on from there lands on the owner's file too.
        let restored = written(render(case.kind, Home, Some(&one), &[]).unwrap());
        assert_eq!(text(&restored), text(&read(case.before)), "{}", case.before);
    }
}

#[test]
fn the_owners_own_switches_are_never_taken_over() {
    // Atlas wants `deploy` off, which the owner already did: no entry of Atlas's, so
    // turning it back on in Atlas leaves the owner's entry where it was.
    for case in CASES {
        let before = read(case.before);
        assert_eq!(
            render(
                case.kind,
                Home,
                Some(&before),
                &["deploy", "legacy-context"]
            )
            .unwrap(),
            Change::Unchanged,
            "{}",
            case.before
        );
    }
}

#[test]
fn a_file_atlas_creates_goes_when_its_entries_do() {
    for (kind, reach) in [
        (ClaudeCode, Home),
        (ClaudeCode, Project),
        (Codex, Home),
        (Codex, Project),
        (OpenCode, Home),
        (Pi, Home),
    ] {
        assert_eq!(
            render(kind, reach, None, &[]).unwrap(),
            Change::Unchanged,
            "{kind}"
        );
        let made = written(render(kind, reach, None, &["pdf-tools"]).unwrap());
        assert!(text(&made).contains("pdf-tools"), "{kind}");
        assert_eq!(
            render(kind, reach, Some(&made), &["pdf-tools"]).unwrap(),
            Change::Unchanged,
            "{kind}"
        );
        assert_eq!(
            render(kind, reach, Some(&made), &[]).unwrap(),
            Change::Remove,
            "{kind}: {}",
            text(&made)
        );
    }
}

#[test]
fn created_files_look_like_the_agents_own() {
    let claude = written(render(ClaudeCode, Project, None, &["pdf-tools"]).unwrap());
    assert_eq!(
        text(&claude),
        "{\n  \"skillOverrides\": {\n    \"pdf-tools\": \"off\"\n  },\n  \"atlas:skillOverrides\": [\n    \"pdf-tools\"\n  ]\n}\n"
    );
    let codex = written(render(Codex, Home, None, &["pdf-tools"]).unwrap());
    assert_eq!(
        text(&codex),
        "[[skills.config]] # atlas:pdf-tools\npath = \"/home/u/.agents/skills/pdf-tools/SKILL.md\"\nenabled = false\n"
    );
    let opencode = written(render(OpenCode, Home, None, &["a", "b"]).unwrap());
    assert_eq!(
        text(&opencode),
        "// Atlas made this file to switch skills off for OpenCode, and deletes it once nothing else is in it.\n\
         {\n  \"$schema\": \"https://opencode.ai/config.json\",\n  \"permission\": {\n    \"skill\": {\n      \
         \"a\": \"deny\", // atlas:a\n      \"b\": \"deny\" // atlas:b\n    }\n  }\n}\n"
    );
    let pi = written(render(Pi, Home, None, &["pdf-tools"]).unwrap());
    assert_eq!(
        text(&pi),
        "{\n  \"skills\": [\n    \"-skills/pdf-tools/SKILL.md\"\n  ],\n  \"atlas:skills\": [\n    \"pdf-tools\"\n  ]\n}\n"
    );
}

#[test]
fn an_entry_the_owner_changed_becomes_theirs() {
    // Claude: the owner turned Atlas's "off" into "name-only" with the /skills menu.
    let claude =
        br#"{"skillOverrides": {"pdf-tools": "name-only"}, "atlas:skillOverrides": ["pdf-tools"]}"#;
    let got = written(render(ClaudeCode, Project, Some(claude), &[]).unwrap());
    assert_eq!(
        text(&got),
        "{\n  \"skillOverrides\": {\n    \"pdf-tools\": \"name-only\"\n  }\n}\n"
    );
    // Codex: the owner flipped Atlas's table on.
    let codex = b"[[skills.config]] # atlas:pdf-tools\nname = \"pdf-tools\"\nenabled = true\n";
    assert_eq!(
        render(Codex, Home, Some(codex), &["pdf-tools"]).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        render(Codex, Home, Some(codex), &[]).unwrap(),
        Change::Unchanged
    );
    // OpenCode: the owner set Atlas's entry to "ask".
    let opencode =
        b"{\"permission\": {\"skill\": {\n  \"pdf-tools\": \"ask\" // atlas:pdf-tools\n}}}";
    assert_eq!(
        render(OpenCode, Home, Some(opencode), &[]).unwrap(),
        Change::Unchanged
    );
    // Pi: the owner removed Atlas's exclusion; the stale name is dropped from the list.
    let pi = br#"{"skills": [], "atlas:skills": ["pdf-tools"]}"#;
    assert_eq!(
        text(&written(render(Pi, Home, Some(pi), &[]).unwrap())),
        "{\n  \"skills\": []\n}\n"
    );
}

#[test]
fn codex_follows_the_skills_folder() {
    let first = written(render(Codex, Home, None, &["pdf-tools"]).unwrap());
    // The folder moved: Atlas's table is updated in place, header comment and all.
    let moved = BTreeSet::from([SwitchOff::at(
        "pdf-tools",
        "/home/u/.codex/skills/pdf-tools/SKILL.md",
    )]);
    let got = written(render_switches(Codex, Home, Some(&first), &moved).unwrap());
    assert_eq!(
        text(&got),
        "[[skills.config]] # atlas:pdf-tools\npath = \"/home/u/.codex/skills/pdf-tools/SKILL.md\"\nenabled = false\n"
    );
    // No folder known: a name entry, which Codex also honours.
    let pdf = by_name(&["pdf-tools"]);
    let named = written(render_switches(Codex, Home, None, &pdf).unwrap());
    assert_eq!(
        text(&named),
        "[[skills.config]] # atlas:pdf-tools\nname = \"pdf-tools\"\nenabled = false\n"
    );
    // For a project's skill a name entry would switch off a global one too: nothing then,
    // and an existing table is kept rather than guessed at.
    assert_eq!(
        render_switches(Codex, Project, None, &pdf).unwrap(),
        Change::Unchanged
    );
    assert_eq!(
        render_switches(Codex, Project, Some(&first), &pdf).unwrap(),
        Change::Unchanged
    );
}

#[test]
fn opencode_keeps_comments_through_add_and_remove() {
    let before = read("opencode/opencode.before.jsonc");
    let after = read("opencode/opencode.after.jsonc");
    for comment in [
        "// Keep the model pinned; the default changes too often.",
        "/* ask before every edit */",
        "// Never in OpenCode.",
    ] {
        assert!(text(&after).contains(comment), "{comment}");
    }
    // Atlas's entries come after the owner's wildcard, so they win.
    let s = text(&after);
    assert!(s.find("\"*\": \"allow\"").unwrap() < s.find("// atlas:pdf-tools").unwrap());
    assert_ne!(before, after);
}

#[test]
fn opencode_makes_the_objects_it_needs() {
    let cases: [(&[u8], &str); 3] = [
        (
            b"{\n  // mine\n  \"model\": \"x\"\n}\n",
            "{\n  // mine\n  \"model\": \"x\",\n  \"permission\": {\n    \"skill\": {\n      \"pdf\": \"deny\" // atlas:pdf\n    }\n  }\n}\n",
        ),
        (
            b"{\n  \"permission\": {\n    \"bash\": \"ask\"\n  }\n}\n",
            "{\n  \"permission\": {\n    \"bash\": \"ask\",\n    \"skill\": {\n      \"pdf\": \"deny\" // atlas:pdf\n    }\n  }\n}\n",
        ),
        (
            b"{\"model\": \"x\", \"permission\": {\"skill\": {}}}",
            "{\"model\": \"x\", \"permission\": {\"skill\": {\n  \"pdf\": \"deny\" // atlas:pdf\n}}}",
        ),
    ];
    for (before, want) in cases {
        let got = written(render(OpenCode, Home, Some(before), &["pdf"]).unwrap());
        assert_eq!(text(&got), want);
        let back = written(render(OpenCode, Home, Some(&got), &[]).unwrap());
        // Objects Atlas's removals empty go with them, so only an owner's empty
        // `"skill": {}` doesn't come back.
        let expect = text(before).replace(", \"permission\": {\"skill\": {}}", "");
        assert_eq!(text(&back), expect);
    }
}

#[test]
fn unparseable_files_are_refused() {
    let cases: [(HarnessKind, &[u8]); 10] = [
        (ClaudeCode, b"{\"skillOverrides\": {\"a\": \"off\"},}"),
        (ClaudeCode, b"// note\n{}"),
        (Pi, b"{\"skills\": [\"a\",]}"),
        (Codex, b"[[skills.config]\npath = 1"),
        (Codex, b"\xff\xfe"),
        (
            OpenCode,
            b"{\"permission\": {\"skill\": {\"a\": \"deny\" \"b\": 1}}}",
        ),
        (OpenCode, b"{\"model\": 'x'}"),
        (OpenCode, b"{ /* never closed"),
        (OpenCode, b"\xff{}"),
        (OpenCode, b"[]"),
    ];
    for (kind, bytes) in cases {
        let got = render(kind, Home, Some(bytes), &["pdf"]);
        assert!(
            got.is_err(),
            "{kind}: {:?} gave {got:?}",
            String::from_utf8_lossy(bytes)
        );
    }
}

#[test]
fn settings_in_a_shape_atlas_doesnt_edit_are_refused() {
    let cases: [(HarnessKind, &[u8], &str); 8] = [
        (
            ClaudeCode,
            b"{\"skillOverrides\": [\"a\"]}",
            "skillOverrides",
        ),
        // One bad value makes Claude Code drop every override in the file.
        (
            ClaudeCode,
            b"{\"skillOverrides\": {\"a\": false}}",
            "skillOverrides.a",
        ),
        (
            ClaudeCode,
            b"{\"atlas:skillOverrides\": \"a\"}",
            "atlas:skillOverrides",
        ),
        // Pi's old shape, which Pi migrates itself.
        (Pi, b"{\"skills\": {\"customDirectories\": []}}", "skills"),
        (Codex, b"skills = 1\n", "skills"),
        (
            Codex,
            b"[skills]\nconfig = [{ name = \"a\", enabled = false }]\n",
            "skills.config",
        ),
        (OpenCode, b"{\"permission\": \"allow\"}", "permission"),
        (
            OpenCode,
            b"{\"permission\": {\"skill\": \"deny\"}}",
            "permission.skill",
        ),
    ];
    for (kind, bytes, at) in cases {
        match render(kind, Home, Some(bytes), &["pdf"]) {
            Err(FormatError::UnexpectedShape { at: got, .. }) => assert_eq!(got, at, "{kind}"),
            other => panic!("{kind}: {other:?}"),
        }
    }
}

#[test]
fn claudes_two_reaches_share_the_format() {
    let before = read("claude/settings.before.json");
    for reach in [Home, Project] {
        assert_eq!(
            text(&written(
                render(ClaudeCode, reach, Some(&before), &OFF).unwrap()
            )),
            text(&read("claude/settings.after.json")),
        );
    }
}

#[test]
fn names_that_arent_skill_names_are_never_written() {
    for (kind, current) in [
        (ClaudeCode, None),
        (Codex, None),
        (OpenCode, None),
        (Pi, None),
    ] {
        let odd = render(kind, Home, current, &["*", "two\nlines", "Caps", ""]).unwrap();
        assert_eq!(odd, Change::Unchanged, "{kind}");
    }
}

#[test]
fn codex_an_owners_entry_for_one_skill_leaves_a_same_named_one_alone() {
    // The owner switched off project A's `deploy` by its path; Atlas wants the global
    // `deploy` off, which that entry doesn't cover.
    let owner =
        b"[[skills.config]]\npath = \"/home/u/Projects/a/.agents/skills/deploy/SKILL.md\"\nenabled = false\n";
    let got = written(render(Codex, Home, Some(owner), &["deploy"]).unwrap());
    assert!(
        text(&got).ends_with(
            "[[skills.config]] # atlas:deploy\npath = \"/home/u/.agents/skills/deploy/SKILL.md\"\nenabled = false\n"
        ),
        "{}",
        text(&got)
    );
}

#[test]
fn codex_switches_same_named_skills_off_one_by_one() {
    let global = SwitchOff::at("deploy", "/home/u/.agents/skills/deploy/SKILL.md");
    let a = SwitchOff::at(
        "deploy",
        "/home/u/Projects/a/.agents/skills/deploy/SKILL.md",
    );
    let b = SwitchOff::at(
        "deploy",
        "/home/u/Projects/b/.agents/skills/deploy/SKILL.md",
    );
    let table = |s: &SwitchOff| {
        format!(
            "[[skills.config]] # atlas:deploy\npath = \"{}\"\nenabled = false\n",
            s.path.as_ref().unwrap().display()
        )
    };
    let all = BTreeSet::from([global.clone(), a.clone(), b.clone()]);
    let made = written(render_switches(Codex, Home, None, &all).unwrap());
    let want = format!("{}\n{}\n{}", table(&global), table(&a), table(&b));
    assert_eq!(text(&made), want);
    assert_eq!(
        render_switches(Codex, Home, Some(&made), &all).unwrap(),
        Change::Unchanged
    );
    // Back on for project A only: its table goes, the other two stay.
    let without_a = BTreeSet::from([global.clone(), b.clone()]);
    let got = written(render_switches(Codex, Home, Some(&made), &without_a).unwrap());
    assert_eq!(text(&got), format!("{}\n{}", table(&global), table(&b)));
    // A duplicate of one of Atlas's tables (a hand merge) is dropped, not the other skill's.
    let doubled = format!("{}\n{}", text(&made), table(&b));
    assert_eq!(
        text(&written(
            render_switches(Codex, Home, Some(doubled.as_bytes()), &all).unwrap()
        )),
        want
    );
    assert_eq!(
        render_switches(Codex, Home, Some(&made), &BTreeSet::new()).unwrap(),
        Change::Remove
    );
}

#[test]
fn name_keyed_formats_write_one_entry_per_name_whatever_the_paths() {
    let two = BTreeSet::from([
        SwitchOff::at("deploy", "/home/u/.agents/skills/deploy/SKILL.md"),
        SwitchOff::at(
            "deploy",
            "/home/u/Projects/a/.agents/skills/deploy/SKILL.md",
        ),
    ]);
    for kind in [ClaudeCode, OpenCode, Pi] {
        let got = written(render_switches(kind, Home, None, &two).unwrap());
        let one = written(render_switches(kind, Home, None, &by_name(&["deploy"])).unwrap());
        assert_eq!(text(&got), text(&one), "{kind}");
    }
}
