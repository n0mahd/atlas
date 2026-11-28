//! `atlas/placements.toml` on a machine: per-agent off, only-projects, switches and an
//! invalid file (decision 0027).

mod common;

use std::collections::BTreeSet;
use std::fs;

use atlas_client::{desired, plan};
use atlas_core::kernel::HarnessKind;
use common::{Key, World, exists, link};

const OFF_FOR_OPENCODE: &str = "[global.alpha]\noff = [\"open-code\"]\n";

/// The plan for the world's local copies as they stand.
fn plan_now(w: &World) -> plan::Plan {
    let d = desired::read(Some(&w.copy("knowledge")), Some(&w.copy("public-skills")));
    plan::plan(w.home(), &d, std::slice::from_ref(&w.paths.sources))
}

#[test]
fn a_skill_off_for_opencode_goes_in_codexs_own_folder_and_comes_back() {
    let key = Key::new(1, "Laptop");
    let mut w = World::with(&key, &[("atlas/placements.toml", OFF_FOR_OPENCODE)]);
    let h = w.home().to_path_buf();
    fs::remove_dir_all(h.join(".claude")).unwrap();
    fs::create_dir_all(h.join(".config/opencode")).unwrap();

    w.apply(1, &key);
    // ~/.agents/skills is read by OpenCode too; ~/.codex/skills only by Codex.
    assert_eq!(
        link(&h.join(".codex/skills/alpha")),
        w.copy("knowledge/skills/alpha")
    );
    assert!(!exists(&h.join(".agents/skills/alpha")));
    assert!(!exists(&h.join(".config/opencode/skills/alpha")));
    assert_eq!(
        link(&h.join(".agents/skills/gamma")),
        w.copy("knowledge/skills/gamma"),
        "skills that aren't off stay where they were"
    );
    assert!(plan_now(&w).switches.is_empty(), "nobody to switch off");

    // On again: back in the shared folder, and gone from Codex's own.
    w.change_knowledge(&[("atlas/placements.toml", None)]);
    w.apply(2, &key);
    assert_eq!(
        link(&h.join(".agents/skills/alpha")),
        w.copy("knowledge/skills/alpha")
    );
    assert!(
        !exists(&h.join(".codex/skills/alpha")),
        "Codex would see it twice"
    );
}

#[test]
fn an_unwanted_reader_with_a_setting_is_switched_off() {
    // With Claude Code set up, OpenCode reads ~/.claude/skills too, so Claude's link
    // reaches it; OpenCode's own setting turns the skill off.
    let key = Key::new(1, "Laptop");
    let mut w = World::with(&key, &[("atlas/placements.toml", OFF_FOR_OPENCODE)]);
    let h = w.home().to_path_buf();
    fs::create_dir_all(h.join(".config/opencode")).unwrap();

    w.apply(1, &key);
    assert!(exists(&h.join(".claude/skills/alpha")));
    assert!(exists(&h.join(".codex/skills/alpha")));
    assert!(!exists(&h.join(".agents/skills/alpha")));
    let switches = plan_now(&w).switches;
    assert_eq!(switches.len(), 1, "{switches:?}");
    assert_eq!(switches[0].kind, HarnessKind::OpenCode);
    let file = h.join(".config/opencode/opencode.jsonc");
    assert_eq!(switches[0].file, file);
    assert_eq!(
        switches[0].off.get("alpha"),
        Some(&h.join(".claude/skills/alpha")),
        "the folder OpenCode loads it from"
    );
    let text = fs::read_to_string(&file).unwrap();
    assert!(
        text.contains("\"alpha\": \"deny\"") && text.contains("// atlas:alpha"),
        "{text}"
    );

    // On again: Atlas's entry goes, and so does the file it created.
    w.change_knowledge(&[("atlas/placements.toml", None)]);
    w.apply(2, &key);
    assert!(!exists(&file), "{:?}", fs::read_to_string(&file));
    assert!(exists(&h.join(".agents/skills/alpha")));
}

#[test]
fn an_agent_without_a_setting_is_reported() {
    let key = Key::new(1, "Laptop");
    let mut w = World::with(
        &key,
        &[(
            "atlas/placements.toml",
            "[global.alpha]\noff = [\"grok\"]\n",
        )],
    );
    let h = w.home().to_path_buf();
    fs::create_dir_all(h.join(".grok")).unwrap();

    let pass = w.apply(1, &key);
    assert!(exists(&h.join(".claude/skills/alpha")));
    assert!(
        pass.problems
            .iter()
            .any(|p| p.starts_with("skill alpha is off for Grok Build, but Atlas can't keep it")),
        "{:?}",
        pass.problems
    );
}

#[test]
fn a_skill_used_only_in_projects_gets_no_home_links() {
    let key = Key::new(1, "Laptop");
    let mut w = World::with(
        &key,
        &[(
            "atlas/placements.toml",
            "[global.gamma]\nonly-projects = [\"shop\"]\n",
        )],
    );
    let h = w.home().to_path_buf();
    w.apply(1, &key);
    for dir in [".claude/skills", ".agents/skills", ".codex/skills"] {
        assert!(!exists(&h.join(dir).join("gamma")), "{dir}");
    }
    assert!(exists(&h.join(".claude/skills/alpha")));
}

#[test]
fn an_invalid_placements_file_changes_no_skill_links() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    w.apply(1, &key);
    assert!(exists(&h.join(".claude/skills/gamma")));

    // A typo, and gamma deleted: without the file Atlas would unlink gamma.
    w.change_knowledge(&[
        (
            "atlas/placements.toml",
            Some("[global.alpha]\noff = [\"codx\"]\n"),
        ),
        ("skills/gamma/SKILL.md", None),
        ("AGENTS.md", Some("# Global, changed\n")),
    ]);
    fs::remove_file(h.join(".codex/AGENTS.md")).unwrap();
    let pass = w.apply(2, &key);
    assert!(
        pass.problems
            .iter()
            .any(|p| p.contains("atlas/placements.toml is invalid")
                && p.contains("no skill links, switches or Claude Code setting change")),
        "{:?}",
        pass.problems
    );
    for dir in [".claude/skills", ".agents/skills"] {
        let names: BTreeSet<String> = fs::read_dir(h.join(dir))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(
            names,
            BTreeSet::from(["alpha".into(), "beta".into(), "gamma".into()]),
            "{dir}: links kept as they were"
        );
    }
    assert!(
        exists(&h.join(".codex/AGENTS.md")),
        "instructions still apply"
    );
    assert_eq!(w.state.last_applied, Some(2));
}

#[test]
fn one_reader_for_the_whole_placements_file() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    w.apply(1, &key);
    let settings = fs::read_to_string(h.join(".claude/settings.json")).unwrap();

    // A typo in [instructions] is one problem, and changes no skill link or setting.
    w.change_knowledge(&[
        (
            "atlas/placements.toml",
            Some("[instructions]\nclaude-reads-agents-md = \"sometimes\"\n"),
        ),
        ("skills/gamma/SKILL.md", None),
    ]);
    let pass = w.apply(2, &key);
    let about: Vec<&String> = pass
        .problems
        .iter()
        .filter(|p| p.contains("placements.toml"))
        .collect();
    assert_eq!(about.len(), 1, "{:?}", pass.problems);
    assert!(
        exists(&h.join(".claude/skills/gamma")),
        "links stay as they were"
    );
    assert_eq!(
        fs::read_to_string(h.join(".claude/settings.json")).unwrap(),
        settings
    );

    // A link where the file should be is refused the same way, not followed.
    let remote = w.knowledge_remote();
    fs::remove_file(remote.join("atlas/placements.toml")).unwrap();
    std::os::unix::fs::symlink("../AGENTS.md", remote.join("atlas/placements.toml")).unwrap();
    w.change_knowledge(&[]);
    let pass = w.apply(3, &key);
    assert!(
        pass.problems
            .iter()
            .any(|p| p.contains("placements.toml") && p.contains("no skill links")),
        "{:?}",
        pass.problems
    );
    assert!(exists(&h.join(".claude/skills/gamma")));
}
