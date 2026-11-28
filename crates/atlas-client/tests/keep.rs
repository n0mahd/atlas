//! What atlasd replaces or removes without the new content holding it byte for byte is
//! copied first, to `~/.local/state/atlas-client/replaced/<UTC date>/<path in the home>`,
//! and the pass (its printed lines and the report sent to the hub) names each copy. When
//! no copy can be made, the original stays and the pass says so.

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use atlas_core::distribution::{KeptCopy, Report};
use atlas_harness::claude_project_memory;
use common::{Key, World, exists, link, signed, write};

const GLOBAL: &str = "# Global\nUse rg.\nBe brief.\n";
/// Where a pass keeps today's copies: the test passes run at second 1 of 1970.
const DAY: &str = ".local/state/atlas-client/replaced/1970-01-01";

fn world(key: &Key) -> World {
    World::with(key, &[("AGENTS.md", GLOBAL)])
}

/// Every file under `dir`, relative to it.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, at: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir.join(at)) else {
            return;
        };
        for e in entries {
            let e = e.unwrap();
            let rel = at.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                walk(dir, &rel, out);
            } else {
                out.push(rel);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, Path::new(""), &mut out);
    out.sort();
    out
}

fn kept(original: &str) -> KeptCopy {
    KeptCopy {
        original: format!("~/{original}"),
        copy: format!("~/{DAY}/{original}"),
    }
}

#[test]
fn an_own_agents_md_in_another_order_is_kept_before_it_becomes_the_link() {
    let key = Key::new(1, "Laptop");
    let mut w = world(&key);
    let codex = w.home().join(".codex/AGENTS.md");
    let own = "Be brief.\nUse rg.\n\nBe brief.\n";
    write(&codex, own);

    let pass = w.apply(1, &key);
    assert_eq!(link(&codex), w.copy("knowledge/AGENTS.md"));
    let copy = w.home().join(DAY).join(".codex/AGENTS.md");
    assert_eq!(fs::read_to_string(&copy).unwrap(), own, "byte for byte");
    assert_eq!(pass.report.kept, [kept(".codex/AGENTS.md")]);
    assert!(
        pass.lines.contains(&format!(
            "Kept a copy of ~/.codex/AGENTS.md as ~/{DAY}/.codex/AGENTS.md; copy it back to restore it."
        )),
        "{:?}",
        pass.lines
    );
    let sent: Report = serde_json::from_slice(&serde_json::to_vec(&pass.report).unwrap()).unwrap();
    assert_eq!(sent.kept, pass.report.kept, "the hub reads it back");

    let again = w.apply(2, &key);
    assert_eq!(again.report.kept, []);
    assert_eq!(
        files_under(&w.paths.replaced),
        [PathBuf::from("1970-01-01/.codex/AGENTS.md")],
        "no second copy"
    );
}

#[test]
fn claudes_own_text_is_kept_before_it_becomes_just_the_import() {
    let key = Key::new(1, "Laptop");
    let mut w = world(&key);
    let claude = w.home().join(".claude/CLAUDE.md");
    let own = "Be brief.\n\nUse rg.\nBe brief.\n";
    write(&claude, own);

    let pass = w.apply(1, &key);
    assert_eq!(
        fs::read_to_string(&claude).unwrap(),
        format!("@{}\n", w.copy("knowledge/AGENTS.md").display())
    );
    assert_eq!(
        fs::read_to_string(w.home().join(DAY).join(".claude/CLAUDE.md")).unwrap(),
        own
    );
    assert_eq!(pass.report.kept, [kept(".claude/CLAUDE.md")]);

    assert_eq!(w.apply(2, &key).report.kept, []);
    assert_eq!(
        files_under(&w.paths.replaced),
        [PathBuf::from("1970-01-01/.claude/CLAUDE.md")]
    );
}

#[test]
fn adding_the_import_keeps_a_copy_only_when_the_text_changes_as_written() {
    let key = Key::new(1, "Laptop");
    // Every line stays as it was, the import added after them: nothing to keep.
    let mut w = world(&key);
    write(&w.home().join(".claude/CLAUDE.md"), "Mine.\n");
    let pass = w.apply(1, &key);
    assert_eq!(pass.report.kept, []);
    assert!(!exists(&w.paths.replaced));

    // Windows line endings become plain newlines: the original is kept.
    let mut w = world(&key);
    write(&w.home().join(".claude/CLAUDE.md"), "Mine.\r\n");
    let pass = w.apply(1, &key);
    assert_eq!(pass.report.kept, [kept(".claude/CLAUDE.md")]);
    assert_eq!(
        fs::read(w.home().join(DAY).join(".claude/CLAUDE.md")).unwrap(),
        b"Mine.\r\n"
    );
}

#[test]
fn a_copy_that_cant_be_made_leaves_the_original_and_is_reported() {
    let key = Key::new(1, "Laptop");
    let mut w = world(&key);
    let codex = w.home().join(".codex/AGENTS.md");
    let own = "Be brief.\nUse rg.\n";
    write(&codex, own);
    let state = w.home().join(".local/state/atlas-client");
    fs::create_dir_all(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o500)).unwrap();
    if fs::write(state.join("probe"), "").is_ok() {
        eprintln!("skipped: this user can write into a read-only folder");
        return;
    }

    let release = signed(&w.manifest(1, &[&key]), &key);
    let pass = w.run(&release, false);
    assert!(pass.failed);
    assert_eq!(fs::read_to_string(&codex).unwrap(), own, "left as it is");
    assert!(fs::symlink_metadata(&codex).unwrap().is_file());
    assert!(
        pass.problems.iter().any(|p| p.starts_with(
            "couldn't keep a copy in ~/.local/state/atlas-client/replaced and replace ~/.codex/AGENTS.md"
        ) && p.contains("no copy of it could be kept")
            && p.ends_with("so it was left as it is")),
        "{:?}",
        pass.problems
    );
    assert_eq!(pass.report.problems, pass.problems);
    assert_eq!(pass.report.kept, []);

    // A dry run says what a pass would do.
    let dry = w.run(&release, true);
    assert!(
        dry.lines.iter().any(|l| l.starts_with(
            "would keep a copy in ~/.local/state/atlas-client/replaced and replace ~/.codex/AGENTS.md with a link to"
        )),
        "{:?}",
        dry.lines
    );

    // Writable again, the next pass makes the copy and the link.
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let pass = w.apply(1, &key);
    assert_eq!(pass.report.kept, [kept(".codex/AGENTS.md")]);
    assert_eq!(link(&codex), w.copy("knowledge/AGENTS.md"));
}

#[test]
fn an_exact_copy_of_a_skill_is_adopted_without_a_copy() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let alpha = w.home().join(".claude/skills/alpha");
    write(&alpha.join("SKILL.md"), "---\nname: alpha\n---\n");
    let pass = w.apply(1, &key);
    assert_eq!(link(&alpha), w.copy("knowledge/skills/alpha"));
    assert_eq!(pass.report.kept, []);
    assert!(!exists(&w.paths.replaced), "nothing to keep");
}

#[test]
fn a_memory_folder_that_differs_only_in_an_execute_bit_is_kept_before_it_becomes_the_link() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let shop = w.checkout("shop", "shop", &[("AGENTS.md", "# Shop\n")]);
    let store = w.owner_knowledge().join("memory/projects/shop");
    let note = "---\nname: deploys\n---\nDeploys run from CI.\n";
    write(&store.join("deploys.md"), note);
    w.config.memory_store = Some("~/Knowledge".into());
    let memory = claude_project_memory(w.home(), &shop);
    write(&memory.join("deploys.md"), note);
    fs::set_permissions(memory.join("deploys.md"), fs::Permissions::from_mode(0o755)).unwrap();

    let pass = w.apply(1, &key);
    assert_eq!(link(&memory), store);
    let rel = memory.strip_prefix(w.home()).unwrap();
    let copy = w.home().join(DAY).join(rel).join("deploys.md");
    assert_eq!(fs::read_to_string(&copy).unwrap(), note);
    assert_ne!(
        fs::metadata(&copy).unwrap().permissions().mode() & 0o111,
        0,
        "the execute bit is kept"
    );
    assert_eq!(
        pass.report.kept,
        [kept(rel.to_str().unwrap())],
        "{:?}",
        pass.lines
    );
}

#[test]
fn a_settings_file_in_its_own_layout_is_kept_before_atlas_lays_it_out() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let settings = w.home().join(".claude/settings.json");
    let own = "{\"model\": \"opus\"}";
    write(&settings, own);

    let pass = w.apply(1, &key);
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "opus");
    assert!(v["hooks"]["PreToolUse"].is_array(), "{v}");
    assert_eq!(pass.report.kept, [kept(".claude/settings.json")]);
    assert_eq!(
        fs::read_to_string(w.home().join(DAY).join(".claude/settings.json")).unwrap(),
        own
    );

    // Now in Atlas's layout, a rewrite that only takes Atlas's hook out keeps nothing.
    w.change_knowledge(&[
        ("hooks/guard/hook.toml", None),
        ("hooks/guard/run.sh", None),
    ]);
    let pass = w.apply(2, &key);
    assert_eq!(pass.report.kept, [], "{:?}", pass.lines);
    assert_eq!(
        fs::read_to_string(&settings).unwrap(),
        "{\n  \"model\": \"opus\"\n}\n"
    );
}
