//! Projects on each machine (decision 0027): atlasd finds the owner's checkouts by their
//! remote and links skills inside them, keeps those links out of `git status` with its
//! exclude block, never writes a path git tracks, and switches skills off in the agents'
//! own files.

mod common;

use std::fs;
use std::path::PathBuf;

use atlas_client::checkout;
use common::{Key, World, exists, git, link, snapshot, status, write};

const NOTES: &str = "---\nname: release-notes\n---\n";

fn shop(w: &mut World) -> PathBuf {
    w.checkout(
        "shop",
        "shop",
        &[
            ("AGENTS.md", "# Shop\n"),
            (".agents/skills/release-notes/SKILL.md", NOTES),
        ],
    )
}

#[test]
fn project_skills_get_relative_links_that_git_status_never_shows() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let shop = shop(&mut w);
    write(&shop.join(".git/info/exclude"), "# mine\n*.log\n");

    w.apply(1, &key);
    // Claude Code doesn't read .agents/skills inside a repo; Codex already does.
    let linked = shop.join(".claude/skills/release-notes");
    assert_eq!(
        link(&linked),
        PathBuf::from("../../.agents/skills/release-notes")
    );
    assert!(linked.join("SKILL.md").is_file());
    assert!(!exists(
        &shop.join(".agents/skills/release-notes/release-notes")
    ));
    let exclude = fs::read_to_string(shop.join(".git/info/exclude")).unwrap();
    assert!(exclude.starts_with("# mine\n*.log\n"), "{exclude}");
    assert!(
        checkout::listed(&exclude).contains(".claude/skills/release-notes"),
        "{exclude}"
    );
    assert_eq!(status(&shop), "", "the owner's git status stays clean");

    let before = snapshot(&shop);
    w.apply(2, &key);
    assert_eq!(snapshot(&shop), before, "a second pass changes nothing");
}

#[test]
fn a_checkout_with_another_remote_is_left_alone() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let shop = shop(&mut w);
    git(
        &shop,
        &[
            "remote",
            "set-url",
            "origin",
            "git@git.example:me/other.git",
        ],
    );
    w.apply(1, &key);
    assert!(!exists(&shop.join(".claude")));
    assert!(!exists(&shop.join(".git/info/exclude")) || status(&shop).is_empty());
}

#[test]
fn a_global_skill_used_only_in_a_project_is_linked_there_and_nowhere_else() {
    let key = Key::new(1, "Laptop");
    let mut w = World::with(
        &key,
        &[(
            "atlas/placements.toml",
            "[global.gamma]\nonly-projects = [\"shop\"]\n",
        )],
    );
    let shop = shop(&mut w);
    let other = w.checkout("other", "other", &[("AGENTS.md", "# Other\n")]);
    let h = w.home().to_path_buf();

    w.apply(1, &key);
    let copy = w.copy("knowledge/skills/gamma");
    assert_eq!(link(&shop.join(".claude/skills/gamma")), copy);
    assert_eq!(
        link(&shop.join(".agents/skills/gamma")),
        copy,
        "Codex reads the project's .agents/skills"
    );
    for dir in [".claude/skills", ".agents/skills"] {
        assert!(!exists(&h.join(dir).join("gamma")), "~/{dir}");
        assert!(!exists(&other.join(dir).join("gamma")), "other/{dir}");
    }
    assert_eq!(status(&shop), "");
}

#[test]
fn a_path_git_tracks_is_never_written() {
    let key = Key::new(1, "Laptop");
    let mut w = World::with(
        &key,
        &[(
            "atlas/placements.toml",
            "[global.gamma]\nonly-projects = [\"shop\"]\n",
        )],
    );
    let shop = w.checkout(
        "shop",
        "shop",
        &[(".claude/skills/gamma/SKILL.md", "the team's own gamma\n")],
    );
    // Deleted in the working tree but still tracked: a link there would be a change.
    fs::remove_dir_all(shop.join(".claude/skills/gamma")).unwrap();
    let before = status(&shop);

    let pass = w.apply(1, &key);
    assert!(!exists(&shop.join(".claude/skills/gamma")));
    assert!(
        pass.problems
            .iter()
            .any(|p| p
                .ends_with("/shop/.claude/skills/gamma is tracked by git, so Atlas left it alone")),
        "{:?}",
        pass.problems
    );
    assert_eq!(status(&shop), before, "nothing but the owner's own change");
}

#[test]
fn turning_a_skill_off_removes_only_the_links_atlas_listed() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let shop = shop(&mut w);
    w.apply(1, &key);
    // The owner's own link to the same skill under another name, outside the block.
    std::os::unix::fs::symlink(
        "../../.agents/skills/release-notes",
        shop.join(".claude/skills/notes"),
    )
    .unwrap();

    w.change_knowledge(&[(
        "atlas/placements.toml",
        Some("[project.shop.release-notes]\noff = [\"claude-code\"]\n"),
    )]);
    w.apply(2, &key);
    assert!(!exists(&shop.join(".claude/skills/release-notes")));
    assert!(
        exists(&shop.join(".claude/skills/notes")),
        "the owner's stays"
    );
    let exclude = fs::read_to_string(shop.join(".git/info/exclude")).unwrap();
    assert!(
        checkout::listed(&exclude).contains(".claude/skills/release-notes"),
        "entries are only ever added: {exclude}"
    );
}

#[test]
fn project_switches_go_in_codexs_home_file_and_the_repos_local_settings_and_come_back_out() {
    // The team committed a link so Claude Code sees the skill; Codex reads .agents/skills.
    // Neither should use it on this owner's machines, so both are switched off.
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let shop = shop(&mut w);
    std::os::unix::fs::symlink("../../.agents/skills/release-notes", {
        fs::create_dir_all(shop.join(".claude/skills")).unwrap();
        shop.join(".claude/skills/release-notes")
    })
    .unwrap();
    git(&shop, &["add", "-A"]);
    git(&shop, &["commit", "--quiet", "-m", "share the skill"]);
    let h = w.home().to_path_buf();
    let codex = h.join(".codex/config.toml");
    let mine = "# my Codex settings\nmodel = \"gpt\"\n";
    write(&codex, mine);
    w.apply(1, &key);
    let local = shop.join(".claude/settings.local.json");
    assert!(!exists(&local), "nothing is off yet");

    w.change_knowledge(&[(
        "atlas/placements.toml",
        Some("[project.shop.release-notes]\noff = [\"codex\", \"claude-code\"]\n"),
    )]);
    let pass = w.apply(2, &key);
    let text = fs::read_to_string(&codex).unwrap();
    let skill = shop.join(".agents/skills/release-notes/SKILL.md");
    assert!(text.starts_with(mine), "{text}");
    assert!(text.contains("# atlas:release-notes"), "{text}");
    assert!(
        text.contains(&format!("path = \"{}\"", skill.display())),
        "{text}"
    );
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&local).unwrap()).unwrap();
    assert_eq!(json["skillOverrides"]["release-notes"], "off", "{json}");
    let exclude = fs::read_to_string(shop.join(".git/info/exclude")).unwrap();
    assert!(checkout::listed(&exclude).contains(".claude/settings.local.json"));
    assert_eq!(status(&shop), "", "{:?}", pass.problems);

    // On again: both files are as they were.
    w.change_knowledge(&[("atlas/placements.toml", None)]);
    w.apply(3, &key);
    assert_eq!(fs::read_to_string(&codex).unwrap(), mine, "byte for byte");
    assert!(!exists(&local), "Atlas created it, so it goes");
    assert_eq!(status(&shop), "");
}

#[test]
fn two_checkouts_with_a_skill_of_one_name_switch_off_only_the_first_in_codex() {
    // Codex's file names Atlas's switch by skill name, so one name can be off once.
    let key = Key::new(1, "Laptop");
    let mut w = World::with(
        &key,
        &[(
            "atlas/placements.toml",
            "[project.shop.release-notes]\noff = [\"codex\"]\n",
        )],
    );
    let first = shop(&mut w);
    let second = w.checkout(
        "shop",
        "shop-2",
        &[(".agents/skills/release-notes/SKILL.md", NOTES)],
    );
    let pass = w.apply(1, &key);
    let text = fs::read_to_string(w.home().join(".codex/config.toml")).unwrap();
    assert!(
        text.contains(
            &first
                .join(".agents/skills/release-notes")
                .display()
                .to_string()
        )
    );
    assert!(!text.contains(&second.display().to_string()), "{text}");
    assert!(
        pass.problems
            .iter()
            .any(|p| p.starts_with("skill release-notes is off for Codex at both")),
        "{:?}",
        pass.problems
    );
}
