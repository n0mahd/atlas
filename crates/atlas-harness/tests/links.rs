//! Skill-link and instruction-file planning, including the conflicts Atlas must
//! report instead of overwriting.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_harness::instructions::{
    InstructionAction, InstructionConflict, InstructionFile, plan_claude_md, plan_instruction_link,
};
use atlas_harness::skills::{Entry, SkillConflict, SkillOp, SkillPlan, plan_skill_links};

const FOLDER: &str = "/u/.claude/skills";
const STORE: &str = "/srv/knowledge";

fn roots() -> Vec<PathBuf> {
    vec![
        PathBuf::from("/srv/knowledge/skills"),
        PathBuf::from("/srv/skills"),
    ]
}

fn p(s: &str) -> PathBuf {
    PathBuf::from(s)
}

fn plan(current: &[(&str, Entry)], desired: &[(&str, &str)]) -> SkillPlan {
    let current: BTreeMap<_, _> = current
        .iter()
        .map(|(n, e)| ((*n).to_owned(), e.clone()))
        .collect();
    let desired: BTreeMap<_, _> = desired
        .iter()
        .map(|(n, s)| ((*n).to_owned(), p(s)))
        .collect();
    plan_skill_links(Path::new(FOLDER), &current, &desired, &roots())
}

#[test]
fn creates_replaces_and_removes_only_managed_links() {
    let out = plan(
        &[
            ("kept", Entry::Link(p("/srv/knowledge/skills/kept"))),
            ("moved", Entry::Link(p("/srv/knowledge/skills/moved"))),
            ("retired", Entry::BrokenLink(p("/srv/skills/retired"))),
            ("hand-made", Entry::Dir),
            ("vendor", Entry::Link(p("/usr/share/vendor/skills/vendor"))),
        ],
        &[
            ("kept", "/srv/knowledge/skills/kept"),
            ("moved", "/srv/skills/moved"),
            ("new", "/srv/skills/new"),
        ],
    );
    assert_eq!(
        out.ops,
        [
            SkillOp::ReplaceLink {
                name: "moved".into(),
                from: p("/srv/knowledge/skills/moved"),
                to: p("/srv/skills/moved"),
            },
            SkillOp::RemoveLink {
                name: "retired".into(),
                target: p("/srv/skills/retired"),
            },
            SkillOp::CreateLink {
                name: "new".into(),
                target: p("/srv/skills/new"),
            },
        ]
    );
    assert!(out.conflicts.is_empty(), "{:?}", out.conflicts);
}

#[test]
fn applying_the_plan_leaves_nothing_to_do() {
    let current = [("old", Entry::Link(p("/srv/skills/old")))];
    let desired = [("a", "/srv/skills/a"), ("b", "/srv/knowledge/skills/b")];
    assert_eq!(plan(&current, &desired).ops.len(), 3);
    let applied = [
        ("a", Entry::Link(p("/srv/skills/a"))),
        ("b", Entry::Link(p("/srv/knowledge/skills/b"))),
    ];
    assert_eq!(plan(&applied, &desired), SkillPlan::default());
}

#[test]
fn relative_links_resolve_against_the_folder() {
    let out = plan(
        &[("rel", Entry::Link(p("../../../srv/skills/rel")))],
        &[("rel", "/srv/skills/rel")],
    );
    assert_eq!(out, SkillPlan::default());
}

#[test]
fn real_folders_files_and_foreign_links_are_conflicts() {
    let out = plan(
        &[
            ("dir", Entry::Dir),
            ("file", Entry::File),
            ("theirs", Entry::Link(p("/opt/other/theirs"))),
            ("lookalike", Entry::Link(p("/srv/skills-old/lookalike"))),
        ],
        &[
            ("dir", "/srv/skills/dir"),
            ("file", "/srv/skills/file"),
            ("theirs", "/srv/skills/theirs"),
            ("lookalike", "/srv/skills/lookalike"),
            ("stray", "/tmp/stray"),
        ],
    );
    assert!(out.ops.is_empty(), "{:?}", out.ops);
    assert_eq!(
        out.conflicts,
        [
            SkillConflict::NotALink { name: "dir".into() },
            SkillConflict::NotALink {
                name: "file".into()
            },
            SkillConflict::ForeignLink {
                name: "lookalike".into(),
                target: p("/srv/skills-old/lookalike"),
            },
            SkillConflict::ForeignLink {
                name: "theirs".into(),
                target: p("/opt/other/theirs"),
            },
            SkillConflict::SourceOutsideRoots {
                name: "stray".into(),
                source: p("/tmp/stray"),
            },
        ]
    );
}

#[test]
fn unmanaged_entries_without_a_desired_skill_are_ignored() {
    let out = plan(
        &[
            ("dir", Entry::Dir),
            ("theirs", Entry::BrokenLink(p("/opt/gone/theirs"))),
        ],
        &[],
    );
    assert_eq!(out, SkillPlan::default());
}

const AGENTS: &str = "/srv/knowledge/AGENTS.md";
/// The store's `AGENTS.md` text in these tests.
const GLOBAL_TEXT: &str = "# Global\nBe brief.\n";

fn link_plan(current: &InstructionFile) -> InstructionAction {
    plan_instruction_link(
        Path::new("/u/.codex/AGENTS.md"),
        current,
        Path::new(AGENTS),
        GLOBAL_TEXT,
        &[p(STORE)],
    )
}

#[test]
fn instruction_link_cases() {
    assert_eq!(
        link_plan(&InstructionFile::Missing),
        InstructionAction::CreateLink { target: p(AGENTS) }
    );
    assert_eq!(
        link_plan(&InstructionFile::Link(p(AGENTS))),
        InstructionAction::Keep
    );
    assert_eq!(
        link_plan(&InstructionFile::Link(p("/srv/knowledge/old/AGENTS.md"))),
        InstructionAction::ReplaceLink {
            from: p("/srv/knowledge/old/AGENTS.md"),
            to: p(AGENTS),
        }
    );
}

#[test]
fn instruction_link_never_overwrites_the_users_file_or_link() {
    assert_eq!(
        link_plan(&InstructionFile::File("my rules\n".into())),
        InstructionAction::Conflict(InstructionConflict::UserFile)
    );
    assert_eq!(
        link_plan(&InstructionFile::Link(p("/opt/dotfiles/AGENTS.md"))),
        InstructionAction::Conflict(InstructionConflict::ForeignLink {
            target: p("/opt/dotfiles/AGENTS.md"),
        })
    );
}

fn claude_plan(current: &InstructionFile) -> InstructionAction {
    plan_claude_md(
        Path::new("/u/.claude/CLAUDE.md"),
        current,
        Path::new(AGENTS),
        GLOBAL_TEXT,
        &[],
        Path::new("/u"),
    )
}

fn fixture(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(path)
}

#[test]
fn claude_md_golden_appends_the_import_and_keeps_the_rest() {
    let before = fs::read_to_string(fixture("claude/CLAUDE.before.md")).unwrap();
    let InstructionAction::Write(out) = claude_plan(&InstructionFile::File(before.clone())) else {
        panic!("expected a write");
    };
    if std::env::var_os("BLESS").is_some() {
        fs::write(fixture("claude/CLAUDE.after.md"), &out).unwrap();
    }
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out,
        fs::read_to_string(fixture("claude/CLAUDE.after.md")).unwrap()
    );
    assert!(out.starts_with(&before));
    assert_eq!(
        claude_plan(&InstructionFile::File(out)),
        InstructionAction::Keep,
        "idempotent"
    );
}

#[test]
fn claude_md_cases() {
    assert_eq!(
        claude_plan(&InstructionFile::Missing),
        InstructionAction::Write(format!("@{AGENTS}\n").into_bytes())
    );
    assert_eq!(
        claude_plan(&InstructionFile::File(format!(
            "notes\n  @{AGENTS}  \nmore\n"
        ))),
        InstructionAction::Keep
    );
    assert_eq!(
        claude_plan(&InstructionFile::Link(p(AGENTS))),
        InstructionAction::Keep
    );
    assert_eq!(
        claude_plan(&InstructionFile::Link(p("/opt/dotfiles/CLAUDE.md"))),
        InstructionAction::Conflict(InstructionConflict::ForeignLink {
            target: p("/opt/dotfiles/CLAUDE.md"),
        }),
        "never writes through someone else's link"
    );
    assert_eq!(
        plan_claude_md(
            Path::new("/u/.claude/CLAUDE.md"),
            &InstructionFile::Missing,
            Path::new("/srv/my knowledge/AGENTS.md"),
            GLOBAL_TEXT,
            &[],
            Path::new("/u"),
        ),
        InstructionAction::Conflict(InstructionConflict::UnimportablePath)
    );
}

#[test]
fn claude_md_repoints_an_import_from_an_adopted_folder() {
    let before = "# Mine\n@~/Notes/AGENTS.md\n@~/Knowledge/AGENTS.md\ntail\n";
    let plan = |text: &str| {
        plan_claude_md(
            Path::new("/u/.claude/CLAUDE.md"),
            &InstructionFile::File(text.to_owned()),
            Path::new(AGENTS),
            GLOBAL_TEXT,
            &[p("/u/Knowledge")],
            Path::new("/u"),
        )
    };
    let InstructionAction::Write(out) = plan(before) else {
        panic!("expected a write")
    };
    let out = String::from_utf8(out).unwrap();
    assert_eq!(
        out,
        format!("# Mine\n@~/Notes/AGENTS.md\n@{AGENTS}\ntail\n"),
        "replaced in place, other imports kept"
    );
    assert_eq!(plan(&out), InstructionAction::Keep, "idempotent");
    let doubled = format!("@{AGENTS}\nx\n@{AGENTS}\n");
    let InstructionAction::Write(once) = plan(&doubled) else {
        panic!("repeats dropped")
    };
    assert_eq!(String::from_utf8(once).unwrap(), format!("@{AGENTS}\nx\n"));
}
