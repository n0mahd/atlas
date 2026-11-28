//! Instruction files on a machine, against a temp home: an agent's own file is swapped for
//! the link (or Claude's `CLAUDE.md` for just the import) only when all its text is already
//! in the global `AGENTS.md`, re-checked when applied; and Claude Code's AGENTS.md option
//! lands in `settings.json` beside Atlas's hooks.

use std::fs;
use std::path::{Path, PathBuf};

use atlas_client::desired::{self, Desired};
use atlas_client::{apply, plan};

struct Machine {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    knowledge: PathBuf,
}

impl Machine {
    fn new(global: &str, harness_markers: &[&str]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let knowledge = tmp.path().join("sources/knowledge");
        fs::create_dir_all(&knowledge).unwrap();
        fs::write(knowledge.join("AGENTS.md"), global).unwrap();
        for m in harness_markers {
            fs::create_dir_all(home.join(m)).unwrap();
        }
        Self {
            _tmp: tmp,
            home,
            knowledge,
        }
    }

    fn store(&self) -> PathBuf {
        self.knowledge.join("AGENTS.md")
    }

    fn desired(&self) -> Desired {
        desired::read(Some(&self.knowledge), None)
    }

    fn plan(&self, desired: &Desired) -> plan::Plan {
        plan::plan(
            &self.home,
            desired,
            &[self.knowledge.parent().unwrap().to_path_buf()],
        )
    }

    fn write(&self, rel: &str, text: &str) -> PathBuf {
        let path = self.home.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

fn link_target(path: &Path) -> Option<PathBuf> {
    fs::read_link(path).ok()
}

#[test]
fn an_own_file_fully_in_global_is_linked_and_one_that_isnt_is_kept_and_reported() {
    let h = Machine::new(
        "# Global\n\n<!-- atlas: from ~/.codex/AGENTS.md -->\n\nUse rg.\n",
        &[".codex", ".pi/agent"],
    );
    let codex = h.write(".codex/AGENTS.md", "Use rg.\n");
    let pi = h.write(".pi/agent/AGENTS.md", "Use rg.\nPi only.\n");
    let d = h.desired();
    let p = h.plan(&d);
    assert!(
        p.conflicts
            .iter()
            .any(|c| c.contains("~/.pi/agent/AGENTS.md has text that isn't in the global")),
        "{:?}",
        p.conflicts
    );
    let (done, failed) = apply::apply(&p, &h.home);
    assert!(failed.is_empty(), "{failed:?}");
    assert!(
        done.iter()
            .any(|l| l.starts_with("replace ~/.codex/AGENTS.md with a link")),
        "{done:?}"
    );
    assert_eq!(link_target(&codex), Some(h.store()));
    assert_eq!(fs::read_to_string(&pi).unwrap(), "Use rg.\nPi only.\n");
    assert!(h.plan(&d).actions.is_empty(), "nothing left to do");
}

#[test]
fn a_file_edited_after_planning_is_kept() {
    let h = Machine::new("# Global\nUse rg.\n", &[".codex"]);
    let codex = h.write(".codex/AGENTS.md", "Use rg.\n");
    let p = h.plan(&h.desired());
    assert_eq!(p.actions.len(), 1, "{:?}", p.actions);
    // The owner adds a line between planning and applying.
    fs::write(&codex, "Use rg.\nNew rule.\n").unwrap();
    let (_, failed) = apply::apply(&p, &h.home);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].contains("changed since the plan"), "{failed:?}");
    assert_eq!(fs::read_to_string(&codex).unwrap(), "Use rg.\nNew rule.\n");

    // The global file loses the line meanwhile: also kept.
    let h = Machine::new("# Global\nUse rg.\n", &[".codex"]);
    let codex = h.write(".codex/AGENTS.md", "Use rg.\n");
    let p = h.plan(&h.desired());
    fs::write(h.store(), "# Global\n").unwrap();
    let (_, failed) = apply::apply(&p, &h.home);
    assert!(
        failed[0].contains("no longer hold all its text"),
        "{failed:?}"
    );
    assert!(fs::symlink_metadata(&codex).unwrap().is_file());
}

#[test]
fn claudes_own_text_in_global_leaves_just_the_import() {
    let h = Machine::new("# Global\nUse rg.\n", &[".claude"]);
    let claude = h.write(".claude/CLAUDE.md", "Use rg.\n");
    let p = h.plan(&h.desired());
    let (_, failed) = apply::apply(&p, &h.home);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(
        fs::read_to_string(&claude).unwrap(),
        format!("@{}\n", h.store().display())
    );

    // Text Global lacks stays, with the import added.
    let h = Machine::new("# Global\n", &[".claude"]);
    let claude = h.write(".claude/CLAUDE.md", "Mine.\n");
    let (_, failed) = apply::apply(&h.plan(&h.desired()), &h.home);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(
        fs::read_to_string(&claude).unwrap(),
        format!("Mine.\n@{}\n", h.store().display())
    );
}

#[test]
fn claudes_agents_md_option_is_written_beside_atlas_hooks() {
    let h = Machine::new("# Global\n", &[".claude"]);
    fs::create_dir_all(h.knowledge.join("atlas")).unwrap();
    fs::write(
        h.knowledge.join("atlas/placements.toml"),
        "[instructions]\nclaude-reads-agents-md = \"alongside\"\n",
    )
    .unwrap();
    let hook = h.knowledge.join("hooks/guard");
    fs::create_dir_all(&hook).unwrap();
    fs::write(
        hook.join("hook.toml"),
        "description = \"Guard\"\nevent = \"pre-tool-use\"\nmatcher = \"Bash\"\nrun = \"run.sh\"\n",
    )
    .unwrap();
    fs::write(hook.join("run.sh"), "#!/bin/sh\n").unwrap();
    h.write(".claude/settings.json", "{\n  \"model\": \"opus\"\n}\n");

    let d = h.desired();
    assert!(d.problems.is_empty(), "{:?}", d.problems);
    let p = h.plan(&d);
    let settings = h.home.join(".claude/settings.json");
    let writes: Vec<_> = p
        .actions
        .iter()
        .filter(|a| matches!(a, plan::Action::WriteFile { path, .. } if *path == settings))
        .collect();
    assert_eq!(writes.len(), 1, "one write carries both: {:?}", p.actions);
    let (_, failed) = apply::apply(&p, &h.home);
    assert!(failed.is_empty(), "{failed:?}");
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
    assert_eq!(v["model"], "opus");
    assert_eq!(
        v["pluginConfigs"]["cc-plugin-agents-md@builtin"]["options"]["instructionFiles"],
        "claude-md-and-agents-md"
    );
    assert!(v["hooks"]["PreToolUse"].is_array(), "{v}");
    assert!(h.plan(&d).actions.is_empty(), "nothing left to do");
}

#[test]
fn without_a_choice_claudes_setting_is_left_alone() {
    let h = Machine::new("# Global\n", &[".claude"]);
    h.write(".claude/CLAUDE.md", &format!("@{}\n", h.store().display()));
    let before = "{\"pluginConfigs\":{\"cc-plugin-agents-md@builtin\":{\"options\":{\"instructionFiles\":\"claude-md\"}}}}\n";
    h.write(".claude/settings.json", before);
    assert_eq!(h.plan(&h.desired()).actions, []);

    // An [instructions] table that doesn't parse is reported; the rest still plans.
    fs::create_dir_all(h.knowledge.join("atlas")).unwrap();
    fs::write(
        h.knowledge.join("atlas/placements.toml"),
        "[instructions]\nclaude-reads-agents-md = \"sometimes\"\n",
    )
    .unwrap();
    let d = h.desired();
    assert!(d.claude_agents_md.is_none());
    assert_eq!(d.problems.len(), 1, "{:?}", d.problems);
}
