//! The home plan on a home with every harness set up and every kind of entry Atlas meets,
//! recorded as text. With no placements the plan must stay exactly this: per-skill
//! placement solving (decision 0027) replaced the one-folder-per-harness loop without
//! changing a single action. Regenerate with `BLESS=1` only for a change that means to.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use atlas_client::desired;
use atlas_client::plan::{self, Action, Plan};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

const HOOK: &str =
    "description = \"Guard\"\nevent = \"pre-tool-use\"\nmatcher = \"Bash\"\nrun = \"run.sh\"\n";

/// A home with every harness, a store, an adopted folder and the entries Atlas must
/// create, keep, repoint, remove or refuse.
fn fixture(root: &Path) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
    let home = root.join("home");
    let sources = home.join(".local/share/atlas/sources");
    let knowledge = sources.join("knowledge");
    let public = sources.join("public-skills");
    let adopt = root.join("old");
    for marker in [
        ".claude",
        ".codex",
        ".config/opencode",
        ".pi/agent",
        ".gemini",
        ".config/amp",
        ".config/muse",
        ".grok",
        ".t3",
        ".config/herdr",
    ] {
        fs::create_dir_all(home.join(marker)).unwrap();
    }
    write(&knowledge.join("AGENTS.md"), "# Global\n");
    for s in ["alpha", "gamma", "moved", "kept"] {
        write(
            &knowledge.join(format!("skills/{s}/SKILL.md")),
            &format!("---\nname: {s}\n---\n"),
        );
    }
    write(&knowledge.join("hooks/guard/hook.toml"), HOOK);
    write(&knowledge.join("hooks/guard/run.sh"), "#!/bin/sh\nexit 0\n");
    write(&public.join("beta/SKILL.md"), "---\nname: beta\n---\n");
    fs::create_dir_all(adopt.join("moved")).unwrap();

    // ~/.claude/skills: the owner's own folder, a foreign link, a kept link, a stale one.
    let claude = home.join(".claude/skills");
    write(&claude.join("alpha/SKILL.md"), "mine\n");
    fs::create_dir_all(&claude).unwrap();
    symlink("/opt/elsewhere/beta", claude.join("beta")).unwrap();
    symlink(knowledge.join("skills/kept"), claude.join("kept")).unwrap();
    symlink(knowledge.join("skills/retired"), claude.join("retired")).unwrap();
    // ~/.agents/skills: an adopted link to repoint.
    fs::create_dir_all(home.join(".agents/skills")).unwrap();
    symlink(adopt.join("moved"), home.join(".agents/skills/moved")).unwrap();
    // ~/.codex/skills: an old managed link Atlas no longer uses, left alone.
    fs::create_dir_all(home.join(".codex/skills")).unwrap();
    symlink(adopt.join("moved"), home.join(".codex/skills/moved")).unwrap();
    // ~/.pi/agent/skills is a link to another folder.
    fs::create_dir_all(root.join("pi-skills")).unwrap();
    symlink(root.join("pi-skills"), home.join(".pi/agent/skills")).unwrap();

    write(&home.join(".claude/CLAUDE.md"), "# Mine\n");
    write(
        &home.join(".claude/settings.json"),
        "{\n  \"theme\": \"dark\",\n  \"hooks\": {\"Stop\": [{\"hooks\": [{\"type\": \"command\", \"command\": \"mine.sh\"}]}]}\n}\n",
    );
    write(&home.join(".config/opencode/AGENTS.md"), "my rules\n");
    fs::create_dir_all(home.join(".gemini/config")).unwrap();
    symlink(
        "/opt/dotfiles/AGENTS.md",
        home.join(".gemini/config/AGENTS.md"),
    )
    .unwrap();
    (home, sources, knowledge, adopt)
}

/// The plan as text: every action in words (with the bytes of each write) and every
/// conflict, paths shown from `/R` for the temp root.
fn render(plan: &Plan, home: &Path, root: &Path) -> String {
    let mut out = String::new();
    for a in &plan.actions {
        out.push_str(&a.describe(home));
        out.push('\n');
        if let Action::WriteFile { bytes, .. } | Action::WriteInstruction { bytes, .. } = a {
            for line in String::from_utf8_lossy(bytes).lines() {
                out.push_str("    | ");
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    for c in &plan.conflicts {
        out.push_str("conflict: ");
        out.push_str(c);
        out.push('\n');
    }
    out.replace(&root.display().to_string(), "/R")
}

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn with_nothing_off_the_home_plan_is_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (home, sources, knowledge, adopt) = fixture(&root);
    let desired = desired::read(Some(&knowledge), Some(&sources.join("public-skills")));
    let got = render(
        &plan::plan(&home, &desired, &[sources, adopt]),
        &home,
        &root,
    );
    let file = golden("plan-nothing-off.txt");
    if std::env::var_os("BLESS").is_some() {
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, &got).unwrap();
    }
    let want = fs::read_to_string(&file).unwrap();
    assert_eq!(got, want, "the plan changed:\n{got}");
}
