//! Import moves (decision 0027): a real skill folder that is an exact copy of what Atlas
//! now holds becomes Atlas's link; anything else stays where it is.

mod common;

use std::fs;

use atlas_client::keep::Keeper;
use atlas_client::{apply, desired, plan};
use common::{Key, World, exists, link, write};

const ALPHA: &str = "---\nname: alpha\n---\n";

fn plan_now(w: &World) -> plan::Plan {
    let d = desired::read(Some(&w.copy("knowledge")), Some(&w.copy("public-skills")));
    plan::plan(w.home(), &d, std::slice::from_ref(&w.paths.sources))
}

#[test]
fn an_exact_copy_becomes_the_link_and_edited_copies_stay() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    write(&h.join(".claude/skills/alpha/SKILL.md"), ALPHA);
    write(&h.join(".claude/skills/gamma/SKILL.md"), "my own edit\n");
    write(&h.join(".agents/skills/alpha/SKILL.md"), ALPHA);
    write(&h.join(".agents/skills/alpha/notes.md"), "only here\n");

    let pass = w.apply(1, &key);
    assert_eq!(
        link(&h.join(".claude/skills/alpha")),
        w.copy("knowledge/skills/alpha")
    );
    let left: Vec<_> = fs::read_dir(h.join(".claude/skills"))
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        left.iter().all(|n| !n.starts_with('.')),
        "nothing left aside: {left:?}"
    );
    assert_eq!(
        fs::read_to_string(h.join(".claude/skills/gamma/SKILL.md")).unwrap(),
        "my own edit\n"
    );
    assert_eq!(
        fs::read_to_string(h.join(".agents/skills/alpha/notes.md")).unwrap(),
        "only here\n",
        "a copy with more in it stays"
    );
    for kept in ["~/.claude/skills/gamma", "~/.agents/skills/alpha"] {
        assert!(
            pass.problems
                .iter()
                .any(|p| p.starts_with(&format!("{kept} is your own folder"))),
            "{kept}: {:?}",
            pass.problems
        );
    }
}

#[test]
fn a_copy_changed_after_planning_is_left_alone() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    w.apply(1, &key);
    fs::remove_file(h.join(".claude/skills/alpha")).unwrap();
    write(&h.join(".claude/skills/alpha/SKILL.md"), ALPHA);

    let plan = plan_now(&w);
    assert!(
        plan.actions
            .iter()
            .any(|a| matches!(a, plan::Action::AdoptDir { path, .. } if *path == h.join(".claude/skills/alpha"))),
        "{:?}",
        plan.actions
    );
    // The owner adds a file between planning and applying.
    write(&h.join(".claude/skills/alpha/later.md"), "new\n");
    let keeper = Keeper::new(&w.paths.replaced, &h, 1);
    let failed = apply::apply(&plan, &h, &keeper).failed;
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("changed since the plan"), "{failed:?}");
    assert!(
        fs::symlink_metadata(h.join(".claude/skills/alpha"))
            .unwrap()
            .is_dir()
    );
    assert_eq!(
        fs::read_to_string(h.join(".claude/skills/alpha/later.md")).unwrap(),
        "new\n"
    );
    assert!(exists(&h.join(".claude/skills/alpha/SKILL.md")));
}
