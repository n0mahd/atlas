//! Claude Code's per-repo memory folders become links into the owner's own knowledge
//! checkout (decision 0027), so new notes go straight to git; a folder with notes Atlas
//! doesn't hold yet stays where it is.

mod common;

use std::fs;
use std::path::PathBuf;

use atlas_harness::claude_project_memory;
use common::{Key, World, exists, link, write};

const NOTE: &str = "---\nname: deploys\n---\nDeploys run from CI.\n";

/// A world with a checkout of `shop`, the owner's `~/Knowledge` holding one note for it,
/// and `memory_store` pointing there. Returns the memory folder and its store folder.
fn world(key: &Key) -> (World, PathBuf, PathBuf) {
    let mut w = World::new(key);
    let shop = w.checkout("shop", "shop", &[("AGENTS.md", "# Shop\n")]);
    let knowledge = w.owner_knowledge();
    let store = knowledge.join("memory/projects/shop");
    write(&store.join("deploys.md"), NOTE);
    w.config.memory_store = Some("~/Knowledge".into());
    let memory = claude_project_memory(w.home(), &shop);
    (w, memory, store)
}

#[test]
fn a_missing_memory_folder_becomes_the_link_and_stays_one() {
    let key = Key::new(1, "Laptop");
    let (mut w, memory, store) = world(&key);
    w.apply(1, &key);
    assert_eq!(link(&memory), store);
    assert_eq!(fs::read_to_string(memory.join("deploys.md")).unwrap(), NOTE);
    let pass = w.apply(2, &key);
    assert!(
        pass.lines.iter().all(|l| !l.contains("memory")),
        "{:?}",
        pass.lines
    );
    assert_eq!(link(&memory), store);
}

#[test]
fn a_folder_whose_notes_are_all_in_atlas_becomes_the_link() {
    let key = Key::new(1, "Laptop");
    let (mut w, memory, store) = world(&key);
    write(&memory.join("deploys.md"), NOTE);
    w.apply(1, &key);
    assert_eq!(link(&memory), store);
    let siblings: Vec<_> = fs::read_dir(memory.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(siblings, ["memory"], "nothing left aside");
}

#[test]
fn a_folder_with_notes_atlas_lacks_stays_and_is_reported() {
    let key = Key::new(1, "Laptop");
    let (mut w, memory, _) = world(&key);
    write(
        &memory.join("deploys.md"),
        "---\nname: deploys\n---\nEdited here.\n",
    );
    write(&memory.join("new.md"), "a new note\n");
    let pass = w.apply(1, &key);
    assert!(fs::symlink_metadata(&memory).unwrap().is_dir());
    assert_eq!(
        fs::read_to_string(memory.join("new.md")).unwrap(),
        "a new note\n"
    );
    assert!(
        pass.problems
            .iter()
            .any(|p| p.starts_with("2 notes in ~/.claude/projects/")
                && p.contains("aren't in Atlas yet")),
        "{:?}",
        pass.problems
    );
}

#[test]
fn without_the_owners_knowledge_checkout_nothing_is_linked() {
    // Atlas's own copy is reset to the signed commit every pass, so notes written
    // through a link into it would be lost: no link, and a plain note why.
    let key = Key::new(1, "Laptop");
    let (mut w, memory, _) = world(&key);
    w.config.memory_store = None;
    let pass = w.apply(1, &key);
    assert!(!exists(&memory));
    assert!(
        pass.lines
            .iter()
            .any(|l| l.contains("memory folders weren't linked") && l.contains("memory_store")),
        "{:?}",
        pass.lines
    );

    // An adopted folder inside the owner's checkout names it as well.
    w.config.adopt = vec!["~/Knowledge/skills".into()];
    fs::create_dir_all(w.home().join("Knowledge/skills")).unwrap();
    w.apply(2, &key);
    assert!(exists(&memory));

    // A memory_store that isn't a checkout of the knowledge repo is refused.
    fs::remove_file(&memory).unwrap();
    let elsewhere = w.home().join("Notes");
    fs::create_dir_all(elsewhere.join("memory/projects/shop")).unwrap();
    w.config.memory_store = Some("~/Notes".into());
    let pass = w.apply(3, &key);
    assert!(!exists(&memory));
    assert!(
        pass.lines
            .iter()
            .any(|l| l.contains("~/Notes in client.toml isn't a checkout of the knowledge repo")),
        "{:?}",
        pass.lines
    );
}
