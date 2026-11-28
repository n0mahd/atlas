//! `atlas_fs::outside::scan` on a real temporary home: folders, links into the home, links
//! into a tool root, broken links, Codex system skills, Claude Code plugins, memory and
//! hooks.

use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use atlas_core::catalog::SkillHome;
use atlas_core::kernel::{HarnessKind, Slug};
use atlas_fs::outside::{self, HookOwner, NoteState, Origin, Rules};
use atlas_fs::{Project, Sources};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn skill(dir: &Path, body: &str) {
    write(
        &dir.join("SKILL.md"),
        &format!("---\ndescription: x\n---\n{body}\n"),
    );
}

struct Home {
    _tmp: tempfile::TempDir,
    _tools: tempfile::TempDir,
    sources: Sources,
    rules: Rules,
}

fn home() -> Home {
    let tmp = tempfile::tempdir().unwrap();
    let tools = tempfile::tempdir().unwrap();
    let h = tmp.path().canonicalize().unwrap();
    let k = h.join("Knowledge");
    let shop = h.join("Projects/shop");
    fs::create_dir_all(shop.join(".git")).unwrap();
    write(&shop.join("AGENTS.md"), "# Shop\n");
    fs::create_dir_all(h.join(".codex")).unwrap();

    // Agents' own skill folders.
    let claude = h.join(".claude/skills");
    skill(&claude.join("own"), "mine");
    symlink("/etc/hostname", claude.join("own/leak")).unwrap();
    skill(&shop.join("tools/linked"), "in the shop repo");
    symlink(shop.join("tools/linked"), claude.join("linked")).unwrap();
    let tool_skill = tools.path().join("share/sometool/skills/tooled");
    skill(&tool_skill, "a tool's");
    symlink(&tool_skill, claude.join("tooled")).unwrap();
    skill(&h.join(".cua-driver/skills/dotted"), "cua's");
    symlink(h.join(".cua-driver/skills/dotted"), claude.join("dotted")).unwrap();
    symlink(h.join("nowhere"), claude.join("gone")).unwrap();
    skill(&k.join("skills/fromatlas"), "in atlas");
    symlink(k.join("skills/fromatlas"), claude.join("fromatlas")).unwrap();
    // Codex sees `own` through a link to Claude's folder: one skill, two agents.
    fs::create_dir_all(h.join(".codex/skills")).unwrap();
    symlink(claude.join("own"), h.join(".codex/skills/own")).unwrap();
    // Clashes with Atlas's skills.
    skill(&claude.join("same"), "identical");
    skill(&k.join("skills/same"), "identical");
    skill(&claude.join("diff"), "theirs");
    skill(&k.join("skills/diff"), "atlas's");
    skill(&claude.join("kept"), "left outside");
    write(
        &k.join("atlas/placements.toml"),
        "[keep]\nitems = [\"skill:~/.claude/skills/kept\"]\n\n[project.shop.stock]\noff = [\"codex\"]\n",
    );

    // Built in and plugins.
    skill(&h.join(".codex/skills/.system/imagegen"), "codex's");
    let plugin = h.join(".claude/plugins/cache/market/pr/1.0");
    skill(&plugin.join("skills/review"), "plugin");
    write(
        &h.join(".claude/plugins/installed_plugins.json"),
        &format!(
            r#"{{"version":2,"plugins":{{"pr@market":[{{"scope":"user","installPath":"{}"}}]}}}}"#,
            plugin.display()
        ),
    );
    symlink(plugin.join("skills/review"), claude.join("review")).unwrap();

    // Hooks: the owner's, Herdr's and Atlas's own.
    write(
        &h.join(".claude/settings.json"),
        r#"{"hooks":{
          "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"python3 ~/.claude/hooks/no-worktrees.py","timeout":10}]}],
          "Notification":[{"hooks":[{"type":"command","command":"notify-send hi"}]}],
          "Stop":[{"hooks":[
            {"type":"command","command":"~/.claude/hooks/herdr-agent-state.sh"},
            {"type":"command","command":"/x/hooks/guard/run.sh # atlas:guard"}]}]}}"#,
    );

    // Claude Code's memory for the shop project, and for a repo Atlas doesn't know.
    let notes = atlas_harness::claude_project_memory(&h, &shop);
    write(&notes.join("new.md"), "---\nname: new\n---\nNew.\n");
    write(&notes.join("same.md"), "Same.\n");
    write(&notes.join("differs.md"), "Here.\n");
    write(&k.join("memory/projects/shop/same.md"), "Same.\n");
    write(&k.join("memory/projects/shop/differs.md"), "There.\n");
    write(
        &h.join(".claude/projects/-elsewhere-repo/memory/a.md"),
        "A\n",
    );

    // A project skill in Claude's own folder, and one that's also Global.
    skill(&shop.join(".claude/skills/claude-only"), "only claude");
    skill(&shop.join(".agents/skills/stock"), "stock");
    skill(&shop.join(".agents/skills/same"), "identical");

    let sources = Sources {
        home: h.clone(),
        knowledge: k,
        public_skills: None,
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: shop,
        }],
    };
    let mut rules = Rules::default();
    rules.tool_roots.push(tools.path().canonicalize().unwrap());
    Home {
        _tmp: tmp,
        _tools: tools,
        sources,
        rules,
    }
}

#[test]
fn everything_outside_is_classified() {
    let h = home();
    let catalog = atlas_fs::load_catalog(&h.sources);
    let harnesses = atlas_fs::scan_harnesses(&h.sources);
    let out = outside::scan(&h.sources, &catalog, &harnesses, &h.rules);
    assert_eq!(out.problem, None);
    let by = |name: &str| {
        out.skills
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} missing from {:?}", out.skills))
    };

    let own = by("own");
    assert_eq!(own.origin, Origin::OwnFolder);
    assert_eq!(own.key, "skill:~/.claude/skills/own");
    assert!(own.entries.contains(&"~/.codex/skills/own".to_owned()));
    assert!(
        own.kinds.contains(&HarnessKind::ClaudeCode) && own.kinds.contains(&HarnessKind::Codex)
    );
    assert!(own.blocked().is_none() && own.clash.is_none());
    assert_eq!(out.skills.iter().filter(|s| s.name == "own").count(), 1);

    let linked = by("linked");
    assert!(matches!(linked.origin, Origin::Linked { .. }));
    assert_eq!(linked.project.as_ref().map(Slug::as_str), Some("shop"));
    assert!(linked.blocked().is_none());

    let tooled = by("tooled");
    assert_eq!(
        tooled.blocked().as_deref(),
        Some("Installed by sometool"),
        "{tooled:?}"
    );
    assert!(tooled.real.is_none());
    assert_eq!(
        by("dotted").blocked().as_deref(),
        Some("Installed by cua-driver")
    );
    assert!(matches!(by("gone").origin, Origin::Broken { .. }));
    assert!(out.skills.iter().all(|s| s.name != "fromatlas"));
    assert!(out.skills.iter().all(|s| s.name != "review"));

    let same = by("same").clash.clone().unwrap();
    assert!(same.same && same.home == SkillHome::Private);
    assert!(!by("diff").clash.clone().unwrap().same);
    assert!(by("kept").kept && !by("own").kept);

    let built: Vec<(HarnessKind, &str)> = out
        .built_in
        .iter()
        .map(|b| (b.agent, b.name.as_str()))
        .collect();
    assert!(
        built.contains(&(HarnessKind::Codex, "imagegen")),
        "{built:?}"
    );
    assert!(
        built.contains(&(HarnessKind::ClaudeCode, "pr:review")),
        "{built:?}"
    );
    assert!(
        built.contains(&(HarnessKind::ClaudeCode, "review")),
        "{built:?}"
    );

    let hook = |part: &str| out.hooks.iter().find(|h| h.command.contains(part)).unwrap();
    let mine = hook("no-worktrees");
    assert_eq!(mine.owner, HookOwner::Owner);
    assert!(mine.blocked().is_none());
    assert_eq!(mine.name, "no-worktrees");
    assert!(mine.key.starts_with("hook:~/.claude/settings.json#") && !mine.key.contains("python"));
    assert_eq!(hook("herdr").blocked().as_deref(), Some("Added by Herdr"));
    assert!(hook("notify").blocked().unwrap().contains("Notification"));
    assert!(out.hooks.iter().all(|h| !h.command.contains("atlas:guard")));

    let notes = &out.memory[0];
    assert_eq!(notes.project.as_str(), "shop");
    let state = |f: &str| notes.notes.iter().find(|n| n.file == f).unwrap().state;
    assert_eq!(state("new.md"), NoteState::New);
    assert_eq!(state("same.md"), NoteState::Same);
    assert_eq!(state("differs.md"), NoteState::Differs);
    assert_eq!(out.stray_memory.len(), 1);
    assert_eq!(out.stray_memory[0].key, "-elsewhere-repo");

    assert_eq!(out.claude_only.len(), 1);
    assert_eq!(out.claude_only[0].name, "claude-only");
    assert_eq!(out.duplicates.len(), 1);
    assert!(out.duplicates[0].same && out.duplicates[0].name.as_str() == "same");
    assert_eq!(out.switched_off.len(), 1);
    assert_eq!(out.switched_off[0].agents, [HarnessKind::Codex]);
}

#[test]
fn a_memory_folder_already_linked_is_not_outside() {
    let h = home();
    let notes = atlas_harness::claude_project_memory(&h.sources.home, &h.sources.projects[0].root);
    fs::remove_dir_all(&notes).unwrap();
    symlink(h.sources.knowledge.join("memory/projects/shop"), &notes).unwrap();
    let catalog = atlas_fs::load_catalog(&h.sources);
    let out = outside::scan(&h.sources, &catalog, &[], &h.rules);
    assert!(out.memory.is_empty());
}
