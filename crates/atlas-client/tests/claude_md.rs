//! Claude Code's `CLAUDE.md` with an import Atlas adopted from an earlier script: the
//! import moves to Atlas's copy and the pass applies, once.

mod common;

use std::fs;

use atlas_client::pass::Pass;
use common::{Key, World, write};

fn world(key: &Key, claude_md: &str) -> World {
    let mut w = World::with(key, &[("AGENTS.md", "# Global\nBe brief.\n")]);
    w.config.adopt = vec!["~/Knowledge".into()];
    fs::create_dir_all(w.home().join("Knowledge")).unwrap();
    write(&w.home().join(".claude/CLAUDE.md"), claude_md);
    w
}

fn quiet(pass: &Pass, n: u64) {
    assert_eq!(
        pass.lines,
        [format!("Release {n} is applied.")],
        "nothing to do"
    );
}

#[test]
fn an_adopted_import_beside_text_that_is_in_global_becomes_just_the_import() {
    let key = Key::new(1, "Laptop");
    let mut w = world(&key, "@~/Knowledge/AGENTS.md\nBe brief.\n");
    let first = w.apply(1, &key);
    let import = format!("@{}\n", w.copy("knowledge/AGENTS.md").display());
    assert_eq!(
        fs::read_to_string(w.home().join(".claude/CLAUDE.md")).unwrap(),
        import,
        "{:?}",
        first.lines
    );
    quiet(&w.apply(2, &key), 2);
}

#[test]
fn an_adopted_import_beside_the_owners_own_text_is_repointed_in_place() {
    let key = Key::new(1, "Laptop");
    let mut w = world(&key, "# Mine\n@~/Knowledge/AGENTS.md\nOnly mine.\n");
    w.apply(1, &key);
    let import = format!("@{}", w.copy("knowledge/AGENTS.md").display());
    assert_eq!(
        fs::read_to_string(w.home().join(".claude/CLAUDE.md")).unwrap(),
        format!("# Mine\n{import}\nOnly mine.\n")
    );
    quiet(&w.apply(2, &key), 2);
}
