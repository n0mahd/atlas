//! What atlasd installs from a pinned repo must be a regular file or real folder, or a
//! link that stays inside the item's own folder; anything else is skipped and named in
//! the machine's report.

mod common;

use std::os::unix::fs::symlink;

use common::{Key, World, exists, write};

#[test]
fn skills_hooks_and_subagents_that_link_out_of_their_folder_are_skipped_and_reported() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let k = w.knowledge_remote();
    // SKILL.md a link to a hidden file elsewhere in the same repo.
    write(&k.join(".private/notes.md"), "---\nname: hidden\n---\n");
    std::fs::create_dir_all(k.join("skills/hidden-link")).unwrap();
    symlink(
        "../../.private/notes.md",
        k.join("skills/hidden-link/SKILL.md"),
    )
    .unwrap();
    // SKILL.md a link to an absolute path outside the repo.
    std::fs::create_dir_all(k.join("skills/outside-link")).unwrap();
    symlink("/etc/hostname", k.join("skills/outside-link/SKILL.md")).unwrap();
    // A good SKILL.md, but a script inside that links out of the folder.
    write(&k.join("skills/leaky/SKILL.md"), "---\nname: leaky\n---\n");
    std::fs::create_dir_all(k.join("skills/leaky/scripts")).unwrap();
    symlink("/etc/passwd", k.join("skills/leaky/scripts/run.sh")).unwrap();
    // A link that stays inside the folder is fine.
    write(&k.join("skills/tidy/README.md"), "---\nname: tidy\n---\n");
    symlink("README.md", k.join("skills/tidy/SKILL.md")).unwrap();
    // A hook whose script and a subagent that link out.
    write(
        &k.join("hooks/sneaky/hook.toml"),
        "description = \"x\"\nevent = \"stop\"\nrun = \"run.sh\"\n",
    );
    symlink("/bin/true", k.join("hooks/sneaky/run.sh")).unwrap();
    // A hook whose script lacks the execute bit: agents would fail to run it.
    write(
        &k.join("hooks/plain/hook.toml"),
        "description = \"x\"\nevent = \"stop\"\nrun = \"check.sh\"\n",
    );
    std::fs::write(k.join("hooks/plain/check.sh"), "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::create_dir_all(k.join("agents")).unwrap();
    symlink("/etc/hostname", k.join("agents/spy.md")).unwrap();
    w.change_knowledge(&[]);

    let pass = w.apply(1, &key);
    let h = w.home().to_path_buf();
    for skipped in ["hidden-link", "outside-link", "leaky"] {
        assert!(
            !exists(&h.join(".claude/skills").join(skipped)),
            "{skipped} must not be installed"
        );
    }
    assert!(
        exists(&h.join(".claude/skills/tidy")),
        "an inside link is fine"
    );
    assert!(
        exists(&h.join(".claude/skills/alpha")),
        "the rest still install"
    );
    let said = |part: &str| pass.problems.iter().any(|p| p.contains(part));
    assert!(
        said("skill hidden-link skipped: its SKILL.md: it is a link to somewhere else"),
        "{:?}",
        pass.problems
    );
    assert!(said(
        "skill outside-link skipped: its SKILL.md: it is a link to somewhere else"
    ));
    assert!(said(
        "skill leaky skipped: scripts/run.sh inside it is a link to somewhere else"
    ));
    assert!(said(
        "hook sneaky skipped: its script is a link, so machines will skip this hook"
    ));
    assert!(said(
        "subagent spy.md skipped: it is a link to somewhere else"
    ));
    assert!(said(
        "hook plain skipped: its script check.sh isn't executable"
    ));
    let settings = std::fs::read_to_string(h.join(".claude/settings.json")).unwrap_or_default();
    assert!(!settings.contains("atlas:sneaky"), "{settings}");
    assert!(!settings.contains("atlas:plain"), "{settings}");
    assert!(
        settings.contains("atlas:guard"),
        "the executable hook still installs"
    );
}
