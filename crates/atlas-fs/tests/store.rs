use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use atlas_core::catalog::{HookEvent, MemoryKind, SkillHome};
use atlas_core::distribution::{InstalledSkill, InstructionLink};
use atlas_core::kernel::{HarnessKind, Scope, Slug};
use atlas_fs::{Project, Sources, discover_projects, load_catalog, scan_harnesses};

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A home folder with a knowledge repo, a public skills repo and one project.
fn fixture() -> (tempfile::TempDir, Sources) {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let k = home.join("Knowledge");
    write(&k.join("AGENTS.md"), "# Global\nBe kind.\n");
    write(
        &k.join("skills/deploy/SKILL.md"),
        "---\nname: deploy\ndescription: >\n  Ship the app\n---\n# Deploy\n",
    );
    write(&k.join("skills/deploy/scripts/run.sh"), "echo hi\n");
    write(
        &k.join("skills/broken-yaml/SKILL.md"),
        "---\ndescription: [unclosed\n---\nbody\n",
    );
    write(&k.join("skills/empty-folder/notes.txt"), "no skill here\n");
    write(
        &k.join("skills/Bad_Name/SKILL.md"),
        "---\ndescription: x\n---\n",
    );
    write(
        &k.join("memory/user-tz.md"),
        "---\nname: user-tz\ndescription: Time zone\nmetadata:\n  type: user\n  host: all\n---\nEastern. See [[other]].\n",
    );
    write(
        &k.join("memory/hosts/box/disk.md"),
        "---\nname: disk\ndescription: Disk\nmetadata:\n  type: reference\n  host: box\n---\nBig.\n",
    );
    write(&k.join("memory/index.md"), "# Index\n");
    write(
        &k.join("hooks/no-rm/hook.toml"),
        "description = \"Block rm -rf\"\nevent = \"pre-tool-use\"\nmatcher = \"Bash\"\nrun = \"check.sh\"\ntimeout = 10\nharnesses = [\"claude-code\"]\n",
    );
    write(&k.join("hooks/no-rm/check.sh"), "exit 0\n");
    write(
        &k.join("hooks/escape/hook.toml"),
        "description = \"x\"\nevent = \"stop\"\nrun = \"../../secret\"\n",
    );

    let s = home.join("Skills");
    write(&s.join("README.md"), "# Public\n");
    write(
        &s.join("review/SKILL.md"),
        "---\nname: review\ndescription: Review code\n---\n",
    );

    let p = home.join("Projects/shop");
    fs::create_dir_all(p.join(".git")).unwrap();
    write(&p.join("AGENTS.md"), "# Shop\n");
    write(
        &p.join(".agents/skills/stock/SKILL.md"),
        "---\ndescription: Count stock\n---\n",
    );

    let sources = Sources {
        home: home.clone(),
        knowledge: k,
        public_skills: Some(s),
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: p,
        }],
    };
    (tmp, sources)
}

#[test]
fn loads_skills_from_every_home() {
    let (_tmp, sources) = fixture();
    let catalog = load_catalog(&sources);
    let names: Vec<(&str, &SkillHome)> = catalog
        .skills
        .iter()
        .map(|s| (s.name.as_str(), &s.home))
        .collect();
    let shop = SkillHome::Project(Slug::new("shop").unwrap());
    assert_eq!(
        names,
        vec![
            ("review", &SkillHome::Public),
            ("broken-yaml", &SkillHome::Private),
            ("deploy", &SkillHome::Private),
            ("stock", &shop),
        ]
    );
    let deploy = catalog
        .skills
        .iter()
        .find(|s| s.name.as_str() == "deploy")
        .unwrap();
    assert_eq!(deploy.description, "Ship the app");
    assert_eq!(deploy.files, vec!["scripts/run.sh"]);
    assert_eq!(deploy.path, "~/Knowledge/skills/deploy/SKILL.md");
}

#[test]
fn problems_are_reported_not_hidden() {
    let (_tmp, sources) = fixture();
    let problems = load_catalog(&sources).problems;
    let has = |path: &str, words: &str| {
        problems
            .iter()
            .any(|p| p.path.contains(path) && p.message.contains(words))
    };
    assert!(has("broken-yaml/SKILL.md", "valid YAML"), "{problems:#?}");
    assert!(has("skills/empty-folder", "no SKILL.md"), "{problems:#?}");
    assert!(has("skills/Bad_Name", "lowercase"), "{problems:#?}");
    assert!(has("hooks/escape/hook.toml", "own folder"), "{problems:#?}");
}

#[test]
fn reads_memory_instructions_and_hooks() {
    let (_tmp, sources) = fixture();
    let catalog = load_catalog(&sources);

    let memory: Vec<(&str, &MemoryKind, &str)> = catalog
        .memory
        .iter()
        .map(|m| (m.name.as_str(), &m.kind, m.host.as_str()))
        .collect();
    assert_eq!(
        memory,
        vec![
            ("disk", &MemoryKind::Reference, "box"),
            ("user-tz", &MemoryKind::User, "all")
        ]
    );
    assert_eq!(catalog.memory[1].links, vec!["other"]);

    let instructions: Vec<(&Scope, &str)> = catalog
        .instructions
        .iter()
        .map(|i| (&i.scope, i.file_name.as_str()))
        .collect();
    let shop = Scope::Project(Slug::new("shop").unwrap());
    assert_eq!(
        instructions,
        vec![(&Scope::Global, "AGENTS.md"), (&shop, "AGENTS.md")]
    );

    let hook = catalog
        .hooks
        .iter()
        .find(|h| h.name.as_str() == "no-rm")
        .unwrap();
    assert_eq!(hook.manifest.event, HookEvent::PreToolUse);
    assert_eq!(hook.manifest.harnesses, Some(vec![HarnessKind::ClaudeCode]));
    assert!(hook.script_present);
}

#[test]
fn symlinks_in_the_store_never_leak_outside_files() {
    let (tmp, sources) = fixture();
    let secret = tmp.path().join("secret.txt");
    fs::write(&secret, "TOP-SECRET").unwrap();
    // A skill folder that is a link, and a SKILL.md that is a link, both to outside files.
    symlink(tmp.path(), sources.knowledge.join("skills/sneaky")).unwrap();
    fs::create_dir_all(sources.knowledge.join("skills/inner")).unwrap();
    symlink(&secret, sources.knowledge.join("skills/inner/SKILL.md")).unwrap();
    symlink(&secret, sources.knowledge.join("memory/leak.md")).unwrap();

    let catalog = load_catalog(&sources);
    let everything = format!("{catalog:?}")
        + &catalog
            .skills
            .iter()
            .map(|s| s.body.raw().as_str())
            .collect::<String>()
        + &catalog
            .memory
            .iter()
            .map(|m| m.body.raw().as_str())
            .collect::<String>();
    assert!(!everything.contains("TOP-SECRET"));
    assert!(catalog.skills.iter().all(|s| s.name.as_str() != "sneaky"));
    assert!(
        catalog
            .problems
            .iter()
            .any(|p| p.path.ends_with("skills/sneaky")),
        "{:#?}",
        catalog.problems
    );
}

#[test]
fn sees_what_each_harness_reads() {
    let (_tmp, sources) = fixture();
    let home = &sources.home;
    let claude = home.join(".claude/skills");
    fs::create_dir_all(&claude).unwrap();
    symlink(
        sources.knowledge.join("skills/deploy"),
        claude.join("deploy"),
    )
    .unwrap();
    symlink(home.join("Skills/review"), claude.join("review")).unwrap();
    symlink(home.join("gone"), claude.join("old")).unwrap();
    fs::create_dir_all(claude.join("local-only")).unwrap();
    write(
        &home.join(".claude/CLAUDE.md"),
        "Notes\n@~/Knowledge/AGENTS.md\n",
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"guard.sh","timeout":5}]}]}}"#,
    );
    fs::create_dir_all(home.join(".codex")).unwrap();
    symlink(
        sources.knowledge.join("AGENTS.md"),
        home.join(".codex/AGENTS.md"),
    )
    .unwrap();

    let views = scan_harnesses(&sources);
    let claude_view = views
        .iter()
        .find(|v| v.kind == HarnessKind::ClaudeCode)
        .unwrap();
    assert!(claude_view.present);
    let entries = &claude_view.skill_dirs[0].entries;
    assert_eq!(entries.len(), 4);
    assert!(
        matches!(&entries[0], InstalledSkill::FromAtlas { home: SkillHome::Private, name } if name.as_str() == "deploy")
    );
    assert!(matches!(&entries[1], InstalledSkill::Outside { name, .. } if name == "local-only"));
    assert!(matches!(&entries[2], InstalledSkill::Broken { name, .. } if name == "old"));
    assert!(matches!(
        &entries[3],
        InstalledSkill::FromAtlas {
            home: SkillHome::Public,
            ..
        }
    ));
    assert_eq!(claude_view.instructions[0].link, InstructionLink::Imports);
    assert_eq!(claude_view.hooks.len(), 1);
    assert_eq!(claude_view.hooks[0].matcher.as_deref(), Some("Bash"));

    let codex = views.iter().find(|v| v.kind == HarnessKind::Codex).unwrap();
    assert_eq!(codex.instructions[0].link, InstructionLink::Linked);
    let pi = views.iter().find(|v| v.kind == HarnessKind::Pi).unwrap();
    assert!(!pi.present);
    assert_eq!(pi.instructions[0].link, InstructionLink::Missing);
}

#[test]
fn discovers_only_agent_projects() {
    let tmp = tempfile::tempdir().unwrap();
    for (name, files) in [
        ("good", &["AGENTS.md"][..]),
        ("plain", &[][..]),
        ("Bad Name", &["CLAUDE.md"][..]),
    ] {
        let root = tmp.path().join(name);
        fs::create_dir_all(root.join(".git")).unwrap();
        for f in files {
            fs::write(root.join(f), "x").unwrap();
        }
    }
    fs::create_dir_all(tmp.path().join("not-git")).unwrap();
    fs::write(tmp.path().join("not-git/AGENTS.md"), "x").unwrap();

    let (projects, problems) = discover_projects(tmp.path());
    assert_eq!(
        projects.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        vec!["good"]
    );
    assert_eq!(problems.len(), 1);
}

#[test]
fn accepts_what_harnesses_accept() {
    let (_tmp, sources) = fixture();
    let k = &sources.knowledge;
    write(
        &k.join("skills/colons/SKILL.md"),
        "---\nname: colons\ndescription: Use it: always, \"quoted\" too\n---\n",
    );
    write(
        &k.join("memory/hosts/box/prefixed.md"),
        "---\nname: box-prefixed\ndescription: d\nmetadata:\n  type: project\n  host: box\n---\n",
    );
    // A project skill linking to a file elsewhere in the same repo is fine.
    let repo = &sources.projects[0].root;
    write(
        &repo.join("docs/skill.md"),
        "---\ndescription: From docs\n---\nBody\n",
    );
    fs::create_dir_all(repo.join(".agents/skills/linked")).unwrap();
    symlink(
        "../../../docs/skill.md",
        repo.join(".agents/skills/linked/SKILL.md"),
    )
    .unwrap();

    let catalog = load_catalog(&sources);
    let colons = catalog
        .skills
        .iter()
        .find(|s| s.name.as_str() == "colons")
        .unwrap();
    assert_eq!(colons.description, "Use it: always, \"quoted\" too");
    let linked = catalog
        .skills
        .iter()
        .find(|s| s.name.as_str() == "linked")
        .unwrap();
    assert_eq!(linked.description, "From docs");
    assert!(catalog.memory.iter().any(|m| m.name == "box-prefixed"));
    let noisy: Vec<_> = catalog
        .problems
        .iter()
        .filter(|p| {
            p.path.contains("colons") || p.path.contains("linked") || p.path.contains("prefixed")
        })
        .collect();
    assert!(noisy.is_empty(), "{noisy:#?}");
}
