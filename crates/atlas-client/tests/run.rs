//! Whole passes against a temp home: local git repos stand in for the remotes, and
//! releases are signed with a P-256 key made here, exactly as a passkey would.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_client::config::{Config, Paths, State};
use atlas_client::pass::{self, Fetched, Pass};
use atlas_core::kernel::{HarnessKind, TreeHash};
use atlas_core::release::{
    CommitId, FORMAT, Manifest, OwnerKey, Signature, SignedRelease, Source, SourceId,
};
use atlas_verify::encode;
use p256::ecdsa::SigningKey;
use sha2::{Digest as _, Sha256};

const HUB: &str = "https://hub.example";
const RP: &str = "hub.example";

struct Key {
    id: Vec<u8>,
    signing: SigningKey,
    name: &'static str,
}

impl Key {
    fn new(seed: u8, name: &'static str) -> Self {
        Key {
            id: vec![seed; 16],
            signing: SigningKey::from_slice(&Sha256::digest([b'k', seed])).unwrap(),
            name,
        }
    }

    fn owner(&self) -> OwnerKey {
        OwnerKey {
            credential_id: encode(&self.id),
            alg: "es256".into(),
            public_key: encode(self.signing.verifying_key().to_sec1_point(false).as_bytes()),
            name: self.name.into(),
        }
    }

    /// A WebAuthn assertion over `challenge`, as a browser and authenticator make it.
    fn sign(&self, challenge: &[u8]) -> Signature {
        use p256::ecdsa::signature::Signer as _;
        let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
        auth.push(0x05); // user present + verified
        auth.extend_from_slice(&1u32.to_be_bytes());
        let client = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"{HUB}"}}"#,
            encode(challenge)
        );
        let mut message = auth.clone();
        message.extend_from_slice(&Sha256::digest(client.as_bytes()));
        let sig: p256::ecdsa::Signature = self.signing.sign(&message);
        Signature {
            credential_id: encode(&self.id),
            authenticator_data: encode(&auth),
            client_data_json: encode(client.as_bytes()),
            signature: encode(sig.to_der().as_bytes()),
        }
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A repo made of `files`, committed; returns it as a release source.
fn repo(dir: &Path, id: SourceId, files: &[(&str, &str)]) -> Source {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet"]);
    for (path, text) in files {
        write(&dir.join(path), text);
        // Hook scripts are committed executable, as they must be to run.
        if Path::new(path).extension().is_some_and(|e| e == "sh") {
            fs::set_permissions(dir.join(path), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "store"]);
    source(dir, id)
}

/// `dir`'s HEAD as a release source.
fn source(dir: &Path, id: SourceId) -> Source {
    Source {
        id,
        remote: dir.to_string_lossy().into_owned(),
        commit: CommitId::new(git(dir, &["rev-parse", "HEAD"])).unwrap(),
        tree: TreeHash::new(git(dir, &["rev-parse", "HEAD^{tree}"])).unwrap(),
    }
}

struct World {
    _tmp: tempfile::TempDir,
    paths: Paths,
    config: Config,
    state: State,
    sources: Vec<Source>,
    adopt: PathBuf,
}

const HOOK: &str =
    "description = \"Guard\"\nevent = \"pre-tool-use\"\nmatcher = \"Bash\"\nrun = \"run.sh\"\n";

impl World {
    fn new(key: &Key) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        for marker in [".claude", ".codex"] {
            fs::create_dir_all(home.join(marker)).unwrap();
        }
        let knowledge = repo(
            &root.join("remotes/knowledge"),
            SourceId::Knowledge,
            &[
                ("AGENTS.md", "# Global\n"),
                ("skills/alpha/SKILL.md", "---\nname: alpha\n---\n"),
                ("skills/gamma/SKILL.md", "---\nname: gamma\n---\n"),
                ("hooks/guard/hook.toml", HOOK),
                ("hooks/guard/run.sh", "#!/bin/sh\nexit 0\n"),
                ("agents/reviewer.md", "---\nname: reviewer\n---\nReview.\n"),
                ("agents/planner.md", "---\nname: planner\n---\nPlan.\n"),
            ],
        );
        let public = repo(
            &root.join("remotes/public"),
            SourceId::PublicSkills,
            &[
                ("beta/SKILL.md", "---\nname: beta\n---\n"),
                ("alpha/SKILL.md", "---\nname: alpha\n---\n"),
            ],
        );
        let adopt = root.join("old-skills");
        fs::create_dir_all(adopt.join("alpha")).unwrap();
        World {
            _tmp: tmp,
            paths: Paths::new(&home),
            config: Config {
                hub: HUB.into(),
                client_id: "c1".into(),
                token: "t".into(),
                machine: "box".into(),
                adopt: vec![adopt.to_string_lossy().into_owned()],
            },
            state: State {
                owner_keys: vec![key.owner()],
                ..State::default()
            },
            sources: vec![knowledge, public],
            adopt,
        }
    }

    fn home(&self) -> &Path {
        &self.paths.home
    }

    fn copy(&self, rel: &str) -> PathBuf {
        self.paths.sources.join(rel)
    }

    fn manifest(&self, number: u64, keys: &[&Key]) -> Manifest {
        Manifest {
            format: FORMAT,
            number,
            created: 0,
            hub: HUB.into(),
            sources: self.sources.clone(),
            owner_keys: keys.iter().map(|k| k.owner()).collect(),
        }
    }

    /// Runs a pass and keeps the resulting state, as `atlasd run` does.
    fn run(&mut self, release: &SignedRelease, dry_run: bool) -> Pass {
        let pass = pass::run(
            &self.paths,
            &self.config,
            &self.state,
            Fetched::Release(Box::new(release.clone())),
            dry_run,
            1,
        );
        self.state = pass.state.clone();
        pass
    }
}

fn signed(manifest: &Manifest, by: &Key) -> SignedRelease {
    let bytes = serde_json::to_vec(manifest).unwrap();
    SignedRelease {
        number: manifest.number,
        manifest: encode(&bytes),
        signature: by.sign(&Sha256::digest(&bytes)),
    }
}

/// Every entry under `dir`, without following links: link targets, file bytes and modes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let m = fs::symlink_metadata(&p).unwrap();
            let v = if m.file_type().is_symlink() {
                format!("link {}", fs::read_link(&p).unwrap().display())
            } else if m.is_dir() {
                walk(&p, out);
                "dir".to_owned()
            } else {
                format!(
                    "file {:o} {}",
                    m.permissions().mode(),
                    String::from_utf8_lossy(&fs::read(&p).unwrap())
                )
            };
            out.insert(p, v);
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

/// The home, minus Atlas's own cache of local copies.
fn harness_files(w: &World) -> BTreeMap<PathBuf, String> {
    let mut s = snapshot(w.home());
    s.retain(|p, _| !p.starts_with(&w.paths.sources));
    s
}

fn link(path: &Path) -> PathBuf {
    fs::read_link(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

#[test]
fn full_run_installs_everything_and_a_second_run_changes_nothing() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    write(&w.home().join(".claude/CLAUDE.md"), "# Mine\n");
    let release = signed(&w.manifest(1, &[&key]), &key);

    let pass = w.run(&release, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    let h = w.home().to_path_buf();
    for dir in [".claude/skills", ".agents/skills"] {
        assert_eq!(
            link(&h.join(dir).join("alpha")),
            w.copy("knowledge/skills/alpha")
        );
        assert_eq!(
            link(&h.join(dir).join("gamma")),
            w.copy("knowledge/skills/gamma")
        );
        assert_eq!(
            link(&h.join(dir).join("beta")),
            w.copy("public-skills/beta")
        );
        assert!(h.join(dir).join("beta/SKILL.md").is_file());
    }
    assert!(
        !h.join(".config/opencode").exists() && !h.join(".pi").exists(),
        "absent harnesses get nothing"
    );
    assert_eq!(
        link(&h.join(".codex/AGENTS.md")),
        w.copy("knowledge/AGENTS.md")
    );
    let claude_md = fs::read_to_string(h.join(".claude/CLAUDE.md")).unwrap();
    let import = format!("@{}", w.copy("knowledge/AGENTS.md").display());
    assert_eq!(claude_md, format!("# Mine\n{import}\n"));

    let script = w.copy("knowledge/hooks/guard/run.sh");
    let marker = format!("{} # atlas:guard", script.display());
    for config in [".claude/settings.json", ".codex/hooks.json"] {
        let text = fs::read_to_string(h.join(config)).unwrap();
        assert!(text.contains(&marker), "{config}: {text}");
    }
    assert!(
        pass.problems
            .iter()
            .any(|p| p.contains("skill alpha is in both")),
        "{:?}",
        pass.problems
    );
    assert_eq!(w.state.last_applied, Some(1));

    let claude = &pass.report.harnesses[0];
    assert_eq!(claude.skills_from_atlas, 3);
    assert_eq!(claude.instructions, "imports");
    assert_eq!(claude.hooks_managed, 1);
    let codex = &pass.report.harnesses[1];
    assert_eq!(
        (codex.skills_from_atlas, codex.instructions.as_str()),
        (3, "linked"),
        "each skill once: Codex no longer gets a second copy in ~/.codex/skills"
    );
    assert_eq!(pass.report.release, Some(1));

    let before = snapshot(w.home());
    let again = w.run(&release, false);
    assert!(!again.failed, "{:?}", again.problems);
    assert_eq!(again.lines, vec!["Update 1 is applied.".to_owned()]);
    assert_eq!(snapshot(w.home()), before, "the second run is idempotent");
}

#[test]
fn own_folders_and_foreign_links_stay_and_adopted_links_move() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    write(&h.join(".claude/skills/alpha/SKILL.md"), "mine\n");
    fs::create_dir_all(h.join(".agents/skills")).unwrap();
    symlink(w.adopt.join("alpha"), h.join(".agents/skills/alpha")).unwrap();
    fs::create_dir_all(h.join(".agents/skills")).unwrap();
    symlink("/elsewhere/beta", h.join(".agents/skills/beta")).unwrap();

    let pass = w.run(&signed(&w.manifest(1, &[&key]), &key), false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert_eq!(
        fs::read_to_string(h.join(".claude/skills/alpha/SKILL.md")).unwrap(),
        "mine\n"
    );
    assert!(
        pass.problems
            .iter()
            .any(|p| p.contains("~/.claude/skills/alpha is your own folder"))
    );
    assert_eq!(
        link(&h.join(".agents/skills/alpha")),
        w.copy("knowledge/skills/alpha")
    );
    assert_eq!(
        link(&h.join(".agents/skills/beta")),
        PathBuf::from("/elsewhere/beta")
    );
    assert!(
        pass.problems
            .iter()
            .any(|p| p.contains("~/.agents/skills/beta links to /elsewhere/beta")),
        "{:?}",
        pass.problems
    );

    assert_eq!(pass.left_alone.len(), 2, "{:?}", pass.left_alone);
    assert_eq!(
        pass.summary,
        vec![
            "Applied update 1 for Claude Code and Codex: linked 3 skills, 2 subagents and the global instructions, and updated Atlas's hooks.".to_owned(),
            format!(
                "Replaced by links to Atlas's copies: ~/.agents/skills/alpha (was a link to {}). Nothing else of yours was deleted; ~/.claude/CLAUDE.md gained one import line.",
                w.adopt.join("alpha").display()
            ),
            "Left 2 things alone because Atlas doesn't manage them; `atlasd status` lists them."
                .to_owned(),
        ]
    );
}

#[test]
fn the_summary_says_what_was_linked_for_which_agents() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    write(&w.home().join(".claude/CLAUDE.md"), "# Mine\n");
    let release = signed(&w.manifest(1, &[&key]), &key);

    let pass = w.run(&release, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert_eq!(
        pass.summary,
        vec![
            "Applied update 1 for Claude Code and Codex: linked 3 skills, 2 subagents and the global instructions, and updated Atlas's hooks.",
            "Nothing of yours was deleted or replaced; ~/.claude/CLAUDE.md gained one import line.",
        ]
    );

    let again = w.run(&release, false);
    assert_eq!(
        again.summary,
        vec![
            "Applied update 1 for Claude Code and Codex: everything was already in place.",
            "Nothing of yours was deleted or replaced.",
        ]
    );

    let nothing = pass::run(&w.paths, &w.config, &w.state, Fetched::Nothing, false, 2);
    assert_eq!(
        nothing.summary,
        vec!["The hub has no signed update yet, so there is nothing to link."]
    );
}

#[test]
fn the_summary_says_when_no_agent_is_set_up() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    for marker in [".claude", ".codex"] {
        fs::remove_dir_all(w.home().join(marker)).unwrap();
    }
    let pass = w.run(&signed(&w.manifest(1, &[&key]), &key), false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert_eq!(
        pass.summary,
        vec![
            "Update 1 has nothing to link here: no agents are set up for this login yet. atlasd links them on its next run after you install one."
        ]
    );
}

/// Applies release 2 cleanly, then checks that `bad` is refused and changes nothing.
fn assert_refused(make_bad: impl FnOnce(&World, &Key) -> SignedRelease, why: &str) {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let pass = w.run(&signed(&w.manifest(2, &[&key]), &key), false);
    assert!(!pass.failed, "{:?}", pass.problems);
    // A change the bad release would undo, were it applied.
    fs::remove_file(w.home().join(".claude/skills/gamma")).unwrap();

    let bad = make_bad(&w, &key);
    let files = harness_files(&w);
    let state = w.state.clone();
    let pass = w.run(&bad, false);
    assert!(pass.failed, "{why}: not refused");
    assert!(
        pass.problems
            .iter()
            .any(|p| p.starts_with("refused update") && p.contains(why)),
        "{why}: {:?}",
        pass.problems
    );
    assert_eq!(w.state, state, "{why}: state changed");
    assert_eq!(harness_files(&w), files, "{why}: files changed");
    assert_eq!(pass.report.release, Some(2));
}

#[test]
fn a_bad_signature_is_refused() {
    assert_refused(
        |w, key| {
            let mut r = signed(&w.manifest(3, &[key]), key);
            let other = signed(&w.manifest(4, &[key]), key);
            r.signature.signature = other.signature.signature;
            r
        },
        "signature doesn't verify",
    );
}

#[test]
fn an_untrusted_key_is_refused() {
    assert_refused(
        |w, _| {
            let stranger = Key::new(9, "Stranger");
            signed(&w.manifest(3, &[&stranger]), &stranger)
        },
        "isn't an owner key",
    );
}

#[test]
fn a_lower_number_is_refused() {
    assert_refused(|w, key| signed(&w.manifest(1, &[key]), key), "no rollbacks");
}

#[test]
fn a_reused_number_with_other_contents_is_refused() {
    assert_refused(
        |w, key| {
            let mut m = w.manifest(2, &[key]);
            m.created = 99;
            signed(&m, key)
        },
        "reuses the number 2",
    );
}

#[test]
fn a_tree_mismatch_is_refused() {
    assert_refused(
        |w, key| {
            let mut m = w.manifest(3, &[key]);
            m.sources[1].tree = TreeHash::new("0".repeat(40)).unwrap();
            signed(&m, key)
        },
        "but the update pins",
    );
}

#[test]
fn new_keys_are_trusted_only_once_their_release_applied() {
    let old = Key::new(1, "Old");
    let new = Key::new(2, "New");
    let mut w = World::new(&old);

    // Release 1 rotates to the new key but can't apply: its tree doesn't match.
    let mut m = w.manifest(1, &[&new]);
    m.sources[0].tree = TreeHash::new("0".repeat(40)).unwrap();
    assert!(w.run(&signed(&m, &old), false).failed);
    assert_eq!(w.state.owner_keys, vec![old.owner()]);

    let rotate = signed(&w.manifest(1, &[&new]), &old);
    let pass = w.run(&rotate, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert_eq!(w.state.owner_keys, vec![new.owner()]);

    // The same release, fetched again, still applies though its signer is now dropped.
    assert!(!w.run(&rotate, false).failed);

    let pass = w.run(&signed(&w.manifest(2, &[&old]), &old), false);
    assert!(pass.failed);
    assert!(
        pass.problems[0].contains("isn't an owner key"),
        "{:?}",
        pass.problems
    );
    assert_eq!(w.state.last_applied, Some(1));

    assert!(!w.run(&signed(&w.manifest(2, &[&new]), &new), false).failed);
    assert_eq!(w.state.last_applied, Some(2));
}

#[test]
fn a_dry_run_changes_nothing() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let first = signed(&w.manifest(1, &[&key]), &key);

    let before = snapshot(w.home());
    let pass = w.run(&first, true);
    assert!(!pass.failed);
    assert_eq!(snapshot(w.home()), before, "no copies are fetched either");
    assert_eq!(w.state.last_applied, None);

    assert!(!w.run(&first, false).failed);
    fs::remove_file(w.home().join(".codex/AGENTS.md")).unwrap();
    let before = snapshot(w.home());
    let state = w.state.clone();
    let pass = w.run(&signed(&w.manifest(2, &[&key]), &key), true);
    assert_eq!(snapshot(w.home()), before);
    assert_eq!(w.state, state);
    assert!(
        pass.lines
            .iter()
            .any(|l| l.starts_with("would link ~/.codex/AGENTS.md")),
        "{:?}",
        pass.lines
    );
}

#[test]
fn an_unreachable_hub_changes_nothing() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    assert!(!w.run(&signed(&w.manifest(1, &[&key]), &key), false).failed);
    let before = snapshot(w.home());
    let pass = pass::run(
        &w.paths,
        &w.config,
        &w.state,
        Fetched::Unreachable("connection refused".into()),
        false,
        2,
    );
    assert_eq!(snapshot(w.home()), before);
    assert_eq!(pass.state, w.state);
    assert!(
        pass.problems[0].contains("agents keep using the local copies (update 1)"),
        "{:?}",
        pass.problems
    );
}

#[test]
fn config_and_state_are_private() {
    let key = Key::new(1, "Laptop");
    let w = World::new(&key);
    w.config.save(&w.paths.config).unwrap();
    w.state.save(&w.paths.state).unwrap();
    for f in [&w.paths.config, &w.paths.state] {
        assert_eq!(fs::metadata(f).unwrap().permissions().mode() & 0o777, 0o600);
        let dir = f.parent().unwrap();
        assert_eq!(
            fs::metadata(dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    let back = Config::load(&w.paths.config).unwrap();
    assert_eq!(back.adopt, w.config.adopt);
    assert!(!format!("{back:?}").contains("token: \"t\""));
    assert_eq!(State::load(&w.paths.state).unwrap(), w.state);
}

#[test]
fn each_harness_gets_its_skills_in_one_folder() {
    // Codex reads ~/.agents/skills and ~/.codex/skills; links go only in the first.
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".codex")).unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::create_dir_all(home.path().join(".config/opencode")).unwrap();
    let src = home.path().join("src/skills/demo");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("SKILL.md"), "---\ndescription: d\n---\n").unwrap();
    let desired = atlas_client::desired::Desired {
        skills: [("demo".to_owned(), src.clone())].into_iter().collect(),
        ..Default::default()
    };
    let plan = atlas_client::plan::plan(home.path(), &desired, &[home.path().join("src")]);
    let text = format!("{plan:?}");
    assert!(text.contains(".agents/skills"), "{text}");
    assert!(text.contains(".claude/skills"), "{text}");
    assert!(
        !text.contains(".codex/skills"),
        "Codex would see the skill twice: {text}"
    );
    assert!(
        !text.contains("opencode/skills"),
        "OpenCode already reads Claude's and Codex's: {text}"
    );
}

#[test]
fn grok_build_gets_the_global_instructions_and_nothing_twice() {
    // Grok Build reads ~/.agents/skills, ~/.claude/skills and Claude's hooks itself, so with
    // Claude Code set up it only needs its own AGENTS.md link (decisions 0022, 0026).
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    let mine = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine.sh"}]}]}}"#;
    write(&h.join(".grok/hooks/mine.json"), mine);
    let release = signed(&w.manifest(1, &[&key]), &key);

    let pass = w.run(&release, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert!(
        pass.problems.iter().all(|p| !p.contains(".grok")),
        "{:?}",
        pass.problems
    );
    assert_eq!(
        link(&h.join(".grok/AGENTS.md")),
        w.copy("knowledge/AGENTS.md")
    );
    assert!(!h.join(".grok/skills").exists(), "Grok already reads them");
    assert_eq!(
        fs::read_dir(h.join(".grok/hooks")).unwrap().count(),
        1,
        "Claude's copy of the hook reaches Grok, so no atlas.json"
    );
    assert_eq!(
        fs::read_to_string(h.join(".grok/hooks/mine.json")).unwrap(),
        mine
    );
    let grok = pass
        .report
        .harnesses
        .iter()
        .find(|r| r.kind == HarnessKind::Grok)
        .unwrap();
    assert_eq!(
        (grok.instructions.as_str(), grok.hooks_managed),
        ("linked", 0)
    );
}

#[test]
fn grok_build_without_claude_gets_atlas_hooks_in_its_own_file() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    fs::remove_dir_all(h.join(".claude")).unwrap();
    let mine = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"mine.sh"}]}]}}"#;
    write(&h.join(".grok/hooks/mine.json"), mine);
    let release = signed(&w.manifest(1, &[&key]), &key);

    let pass = w.run(&release, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    let atlas = fs::read_to_string(h.join(".grok/hooks/atlas.json")).unwrap();
    let json: serde_json::Value = serde_json::from_str(&atlas).unwrap();
    let command = json["hooks"]["PreToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap();
    assert!(command.ends_with("# atlas:guard"), "{atlas}");
    assert_eq!(json["hooks"]["PreToolUse"][0]["matcher"], "Bash", "{atlas}");
    assert_eq!(
        fs::read_to_string(h.join(".grok/hooks/mine.json")).unwrap(),
        mine,
        "the owner's hook file is untouched"
    );
    let grok = pass
        .report
        .harnesses
        .iter()
        .find(|r| r.kind == HarnessKind::Grok)
        .unwrap();
    assert_eq!(grok.hooks_managed, 1);

    // Claude Code arrives: its copy now reaches Grok, so Atlas's Grok file goes.
    fs::create_dir_all(h.join(".claude")).unwrap();
    let again = w.run(&release, false);
    assert!(!again.failed, "{:?}", again.problems);
    assert!(!h.join(".grok/hooks/atlas.json").exists());
    assert!(h.join(".grok/hooks/mine.json").exists());
}

/// Commits the knowledge remote as it now stands and pins that in the world's release.
fn recommit_knowledge(w: &mut World) {
    let dir = PathBuf::from(&w.sources[0].remote);
    w.sources[0] = repo(&dir, SourceId::Knowledge, &[]);
}

#[test]
fn subagents_are_linked_for_claude_and_pruned_when_removed() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    let first = signed(&w.manifest(1, &[&key]), &key);
    let pass = w.run(&first, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    for name in ["reviewer.md", "planner.md"] {
        assert_eq!(
            link(&h.join(".claude/agents").join(name)),
            w.copy("knowledge/agents").join(name)
        );
    }
    assert!(
        !h.join(".codex/agents").exists(),
        "only Claude Code gets them"
    );
    let claude = &pass.report.harnesses[0];
    assert_eq!(claude.kind, HarnessKind::ClaudeCode);
    assert_eq!(claude.agents_from_atlas, 2);
    assert_eq!(pass.report.harnesses[1].agents_from_atlas, 0);

    let before = snapshot(w.home());
    let again = w.run(&first, false);
    assert!(!again.failed, "{:?}", again.problems);
    assert_eq!(snapshot(w.home()), before, "the second run is idempotent");

    fs::remove_file(PathBuf::from(&w.sources[0].remote).join("agents/planner.md")).unwrap();
    recommit_knowledge(&mut w);
    let next = signed(&w.manifest(2, &[&key]), &key);
    let pass = w.run(&next, false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert!(
        pass.lines
            .iter()
            .any(|l| l.starts_with("remove link ~/.claude/agents/planner.md")),
        "{:?}",
        pass.lines
    );
    assert!(fs::symlink_metadata(h.join(".claude/agents/planner.md")).is_err());
    assert!(h.join(".claude/agents/reviewer.md").is_file());
    assert_eq!(pass.report.harnesses[0].agents_from_atlas, 1);
    let dry = w.run(&next, true);
    assert_eq!(dry.lines, vec!["Nothing to change.".to_owned()]);
}

#[test]
fn a_dry_run_shows_subagent_links() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    assert!(!w.run(&signed(&w.manifest(1, &[&key]), &key), false).failed);
    fs::remove_file(w.home().join(".claude/agents/reviewer.md")).unwrap();
    let before = snapshot(w.home());
    let pass = w.run(&signed(&w.manifest(2, &[&key]), &key), true);
    assert_eq!(snapshot(w.home()), before);
    assert!(
        pass.lines
            .iter()
            .any(|l| l.starts_with("would link ~/.claude/agents/reviewer.md")),
        "{:?}",
        pass.lines
    );
}

#[test]
fn own_subagent_files_stay_and_adopted_links_move() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    write(&h.join(".claude/agents/reviewer.md"), "mine\n");
    write(&w.adopt.join("agents/planner.md"), "old\n");
    symlink(
        w.adopt.join("agents/planner.md"),
        h.join(".claude/agents/planner.md"),
    )
    .unwrap();
    symlink("/elsewhere/helper.md", h.join(".claude/agents/helper.md")).unwrap();

    let pass = w.run(&signed(&w.manifest(1, &[&key]), &key), false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert_eq!(
        fs::read_to_string(h.join(".claude/agents/reviewer.md")).unwrap(),
        "mine\n"
    );
    assert!(
        pass.problems.iter().any(|p| p.contains(
            "~/.claude/agents/reviewer.md is your own folder or file, so Atlas didn't link its subagent reviewer.md"
        )),
        "{:?}",
        pass.problems
    );
    assert_eq!(
        link(&h.join(".claude/agents/planner.md")),
        w.copy("knowledge/agents/planner.md")
    );
    assert_eq!(
        link(&h.join(".claude/agents/helper.md")),
        PathBuf::from("/elsewhere/helper.md"),
        "a link Atlas doesn't manage stays"
    );
}

#[test]
fn no_claude_code_means_no_subagents() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let h = w.home().to_path_buf();
    fs::remove_dir_all(h.join(".claude")).unwrap();
    let pass = w.run(&signed(&w.manifest(1, &[&key]), &key), false);
    assert!(!pass.failed, "{:?}", pass.problems);
    assert!(!h.join(".claude").exists());
    assert!(
        pass.report
            .harnesses
            .iter()
            .all(|r| r.agents_from_atlas == 0)
    );
}

#[test]
fn only_plain_top_level_markdown_files_are_subagents() {
    let tmp = tempfile::tempdir().unwrap();
    let k = tmp.path();
    for f in [
        "good.md",
        "Bad_Name.md",
        ".hidden.md",
        "notes.txt",
        "sub/deep.md",
    ] {
        write(&k.join("agents").join(f), "x\n");
    }
    fs::create_dir_all(k.join("agents/folder.md")).unwrap();
    symlink("/etc/passwd", k.join("agents/link.md")).unwrap();
    let d = atlas_client::desired::read(Some(k), None);
    assert_eq!(d.agents.keys().collect::<Vec<_>>(), ["good.md"]);
    for name in ["Bad_Name.md", "folder.md", "link.md"] {
        assert!(
            d.problems
                .iter()
                .any(|p| p.contains(&format!("subagent {name} skipped"))),
            "{name}: {:?}",
            d.problems
        );
    }
    assert_eq!(d.problems.len(), 3, "{:?}", d.problems);
}

/// `OpenCode` reads both `~/.claude/skills` and `~/.agents/skills`, where the same three skills
/// are linked; the hub's Machines page showed it with six.
#[test]
fn an_agent_reading_two_folders_counts_each_skill_once() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    fs::create_dir_all(w.home().join(".config/opencode")).unwrap();
    let pass = w.run(&signed(&w.manifest(1, &[&key]), &key), false);
    assert!(!pass.failed, "{:?}", pass.problems);
    let opencode = pass
        .report
        .harnesses
        .iter()
        .find(|h| h.kind == HarnessKind::OpenCode)
        .unwrap();
    assert!(opencode.present);
    assert_eq!(opencode.skills_from_atlas, 3);
}

/// A login the hub unpaired, or one never paired, is told so and how to pair, not that
/// the hub couldn't be reached.
#[test]
fn an_unpaired_login_is_told_how_to_pair() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    assert!(!w.run(&signed(&w.manifest(1, &[&key]), &key), false).failed);
    let before = snapshot(w.home());
    let pass = pass::run(&w.paths, &w.config, &w.state, Fetched::Unpaired, false, 2);
    assert!(pass.failed);
    assert_eq!(snapshot(w.home()), before);
    assert_eq!(pass.state, w.state);
    let p = &pass.problems[0];
    assert!(
        p.starts_with("this login isn't paired with https://hub.example any more"),
        "{p}"
    );
    assert!(p.contains("choose Pair on the hub's Machines page"), "{p}");
    assert!(!p.contains("couldn't reach"), "{p}");

    let tmp = tempfile::tempdir().unwrap();
    let fresh = Paths::new(tmp.path());
    for e in [
        Config::load(&fresh.config).unwrap_err(),
        State::load(&fresh.state).unwrap_err(),
    ] {
        assert!(
            e.starts_with("this login isn't paired with a hub yet."),
            "{e}"
        );
        assert!(e.contains("atlasd pair <hub-url> <code>"), "{e}");
    }
}

#[test]
fn a_hook_whose_script_isnt_executable_is_reported_not_installed() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let store = PathBuf::from(&w.sources[0].remote);
    let script = store.join("hooks/guard/run.sh");
    fs::set_permissions(&script, fs::Permissions::from_mode(0o644)).unwrap();
    git(&store, &["commit", "--quiet", "-am", "plain script"]);
    w.sources[0] = source(&store, SourceId::Knowledge);
    let release = signed(&w.manifest(1, &[&key]), &key);

    let pass = w.run(&release, false);
    assert!(
        pass.problems
            .iter()
            .any(|p| p.contains("hook guard skipped") && p.contains("isn't executable")),
        "{:?}",
        pass.problems
    );
    let claude = pass
        .report
        .harnesses
        .iter()
        .find(|r| r.kind == HarnessKind::ClaudeCode)
        .unwrap();
    assert_eq!(claude.hooks_managed, 0);
    let settings = fs::read_to_string(w.home().join(".claude/settings.json")).unwrap_or_default();
    assert!(!settings.contains("atlas:guard"), "{settings}");
}

#[test]
fn a_hook_with_a_linked_script_is_reported_not_installed() {
    let key = Key::new(1, "Laptop");
    let mut w = World::new(&key);
    let store = PathBuf::from(&w.sources[0].remote);
    // The script points at a file elsewhere in the repo, whose changes wouldn't show as
    // changes to this hook.
    write(&store.join("lib/real.sh"), "#!/bin/sh\nexit 0\n");
    fs::set_permissions(store.join("lib/real.sh"), fs::Permissions::from_mode(0o755)).unwrap();
    fs::remove_file(store.join("hooks/guard/run.sh")).unwrap();
    symlink("../../lib/real.sh", store.join("hooks/guard/run.sh")).unwrap();
    // A second hook whose script is real but sources a linked helper.
    write(
        &store.join("hooks/helped/hook.toml"),
        &HOOK.replace("Guard", "Helped"),
    );
    write(
        &store.join("hooks/helped/run.sh"),
        "#!/bin/sh\n. ./helper.sh\n",
    );
    fs::set_permissions(
        store.join("hooks/helped/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink("../../lib/real.sh", store.join("hooks/helped/helper.sh")).unwrap();
    git(&store, &["add", "-A"]);
    git(&store, &["commit", "--quiet", "-m", "linked scripts"]);
    w.sources[0] = source(&store, SourceId::Knowledge);
    let release = signed(&w.manifest(1, &[&key]), &key);

    let pass = w.run(&release, false);
    let has = |text: &str| pass.problems.iter().any(|p| p.contains(text));
    assert!(
        has("hook guard skipped: its script is a link, so machines will skip this hook"),
        "{:?}",
        pass.problems
    );
    assert!(
        has("hook helped skipped: its folder holds a link (helper.sh)"),
        "{:?}",
        pass.problems
    );
    let settings = fs::read_to_string(w.home().join(".claude/settings.json")).unwrap_or_default();
    assert!(
        !settings.contains("atlas:guard") && !settings.contains("atlas:helped"),
        "{settings}"
    );
}
