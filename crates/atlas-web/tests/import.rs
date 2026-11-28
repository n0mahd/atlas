//! Bringing things into Atlas over HTTP, against a temporary home, a temporary git store
//! and a signed-in test hub: skills (links inside skipped, clashes), Claude Code's notes,
//! hooks, Leave it outside, Tidy up, and the refusals.

use std::fs;
use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use atlas_core::catalog::HookEvent;
use atlas_core::kernel::{HarnessKind, Slug};
use atlas_fs::{Project, Sources};
use atlas_web::{Access, AppState, Writer, router};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use tower::ServiceExt;

const ORIGIN: &str = "https://hub.test";
const LOGIN: &str = "owner@github";
const COOKIE: &str = "__Host-atlas";

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn repo(dir: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    for (rel, text) in files {
        write(&dir.join(rel), text);
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "start"]);
}

fn commits(dir: &Path) -> usize {
    git(dir, &["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

fn last_message(dir: &Path) -> String {
    git(dir, &["log", "-1", "--format=%s"]).trim().to_owned()
}

struct Hub {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    knowledge: PathBuf,
    shop: PathBuf,
    app: axum::Router,
    cookie: String,
    csrf: String,
}

/// A home with Claude Code and Codex set up, a knowledge repo and one project.
fn hub() -> Hub {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().canonicalize().unwrap();
    let knowledge = home.join("Knowledge");
    repo(
        &knowledge,
        &[
            ("AGENTS.md", "# Rules\n"),
            ("skills/same/SKILL.md", "---\ndescription: s\n---\nSame\n"),
            (
                "skills/diff/SKILL.md",
                "---\ndescription: d\n---\nAtlas's\n",
            ),
            ("memory/projects/shop/same.md", "Same.\n"),
            ("memory/projects/shop/differs.md", "Atlas's.\n"),
        ],
    );
    let shop = home.join("Projects/shop");
    repo(
        &shop,
        &[
            ("AGENTS.md", "# Shop\n"),
            (
                ".claude/skills/claude-only/SKILL.md",
                "---\ndescription: c\n---\nC\n",
            ),
        ],
    );
    fs::create_dir_all(home.join(".codex")).unwrap();
    let claude = home.join(".claude/skills");
    write(
        &claude.join("own/SKILL.md"),
        "---\ndescription: mine\n---\nMine\n",
    );
    write(&claude.join("own/scripts/run.sh"), "#!/bin/sh\necho hi\n");
    fs::set_permissions(
        claude.join("own/scripts/run.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    symlink("/etc/hostname", claude.join("own/leak")).unwrap();
    write(
        &claude.join("same/SKILL.md"),
        "---\ndescription: s\n---\nSame\n",
    );
    write(
        &claude.join("diff/SKILL.md"),
        "---\ndescription: d\n---\nTheirs\n",
    );
    write(
        &home.join(".claude/settings.json"),
        r#"{"model":"x","hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[
          {"type":"command","command":"python3 ~/.claude/hooks/no-worktrees.py","timeout":10},
          {"type":"command","command":"API_KEY=abc123 notify.sh"}]}]}}"#,
    );
    let notes = atlas_harness_memory(&home, &shop);
    write(
        &notes.join("new.md"),
        "---\nname: new\n---\nNew, kept byte for byte.\n",
    );
    write(&notes.join("same.md"), "Same.\n");
    write(&notes.join("differs.md"), "This machine's.\n");

    let sources = Sources {
        home: home.clone(),
        knowledge: knowledge.clone(),
        public_skills: None,
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: shop.clone(),
        }],
    };
    let writer = Writer::open(ORIGIN, &home.join("state"), Duration::from_secs(60)).unwrap();
    let cookie = writer.auth.sign_in();
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        format!("{COOKIE}={cookie}").parse().unwrap(),
    );
    let csrf = writer.auth.session(&h).unwrap().csrf;
    let state = AppState::new(sources, "test-hub", Vec::new(), Some(writer));
    Hub {
        _tmp: tmp,
        app: router(state, Access::new(vec![LOGIN.to_owned()])),
        home,
        knowledge,
        shop,
        cookie,
        csrf,
    }
}

/// Claude Code's memory folder for a repo (as `atlas_harness::claude_project_memory`).
fn atlas_harness_memory(home: &Path, repo: &Path) -> PathBuf {
    let key: String = repo
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(key).join("memory")
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn send(hub: &Hub, req: Request<Body>) -> (StatusCode, String, String) {
    let res = hub.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let location = res
        .headers()
        .get(header::LOCATION)
        .map_or_else(String::new, |v| v.to_str().unwrap().to_owned());
    let body = to_bytes(res.into_body(), 1 << 22).await.unwrap();
    (
        status,
        location,
        String::from_utf8_lossy(&body).into_owned(),
    )
}

async fn get(hub: &Hub, path: &str) -> String {
    let req = Request::get(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::COOKIE, format!("{COOKIE}={}", hub.cookie))
        .body(Body::empty())
        .unwrap();
    let (status, _, body) = send(hub, req).await;
    assert_eq!(status, StatusCode::OK, "{path}: {body}");
    // Askama writes ' as &#39;.
    body.replace("&#39;", "'")
}

async fn post(hub: &Hub, path: &str, fields: &[(&str, &str)]) -> (StatusCode, String, String) {
    post_as(hub, path, fields, true).await
}

async fn post_as(
    hub: &Hub,
    path: &str,
    fields: &[(&str, &str)],
    signed_in: bool,
) -> (StatusCode, String, String) {
    let mut all = vec![("csrf", hub.csrf.as_str())];
    all.extend_from_slice(fields);
    let body: String = all
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let mut req = Request::post(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if signed_in {
        req = req.header(header::COOKIE, format!("{COOKIE}={}", hub.cookie));
    }
    send(hub, req.body(Body::from(body)).unwrap()).await
}

/// The value of the first `name="key"` field after `marker` on a page.
fn key_after(page: &str, marker: &str) -> String {
    let from = page
        .find(marker)
        .unwrap_or_else(|| panic!("{marker} not on the page"));
    let rest = &page[from..];
    let at = rest.find(r#"name="key" value=""#).unwrap() + r#"name="key" value=""#.len();
    let end = rest[at..].find('"').unwrap();
    rest[at..at + end].to_owned()
}

#[tokio::test]
async fn a_skill_is_copied_and_committed_without_the_links_inside() {
    let hub = hub();
    let page = get(&hub, "/skills").await;
    assert!(page.contains("Outside Atlas") && page.contains("Its own folder in ~/.claude/skills"));
    assert!(
        page.contains("A copy leaves out leak"),
        "left-out entries are named"
    );
    let before = commits(&hub.knowledge);
    let (status, location, body) = post(
        &hub,
        "/import/skill",
        &[("key", "skill:~/.claude/skills/own")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(
        location.starts_with("/skills?done=skipped-1&saved="),
        "{location}"
    );
    let k = &hub.knowledge;
    assert_eq!(commits(k), before + 1);
    assert_eq!(
        last_message(k),
        "Bring own into Atlas from ~/.claude/skills/own"
    );
    assert_eq!(
        fs::read_to_string(k.join("skills/own/SKILL.md")).unwrap(),
        "---\ndescription: mine\n---\nMine\n"
    );
    assert!(fs::symlink_metadata(k.join("skills/own/leak")).is_err());
    let mode = fs::metadata(k.join("skills/own/scripts/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert!(mode & 0o111 != 0, "execute bit kept");
    assert_eq!(git(k, &["status", "--porcelain"]), "");
    // The original stays until atlasd replaces it; the page now calls it Atlas's.
    assert!(hub.home.join(".claude/skills/own/SKILL.md").exists());
    let page = get(&hub, &location).await;
    assert!(page.contains("wasn't copied"), "{page}");
}

#[tokio::test]
async fn home_skills_and_tidy_count_outside_atlas_the_same_way() {
    let hub = hub();
    // A tool's skill, linked into Claude's folder: listed, never offered.
    write(
        &hub.home.join(".local/share/toolbox/skills/tool/SKILL.md"),
        "---\ndescription: t\n---\nT\n",
    );
    symlink(
        hub.home.join(".local/share/toolbox/skills/tool"),
        hub.home.join(".claude/skills/tool"),
    )
    .unwrap();
    // own can come in and diff can be compared; same is already in Atlas; the shop's
    // .claude/skills folder is the one project item.
    let offer = "2 skills on this machine aren't in Atlas; 1 project skill could be tidier.";
    let home = get(&hub, "/").await;
    assert!(home.contains(offer), "{home}");
    let skills = get(&hub, "/skills").await;
    assert!(skills.contains(offer), "{skills}");
    let head = r#"<h2>Outside Atlas</h2><span class="count">2</span>"#;
    assert!(skills.contains(head), "{skills}");
    let outside =
        &skills[skills.find(r#"id="outside""#).unwrap()..skills.find(r#"id="built-in""#).unwrap()];
    assert!(
        outside.contains(">own<") && outside.contains(">diff<"),
        "{outside}"
    );
    assert!(
        !outside.contains(">same<") && !outside.contains(">tool<"),
        "{outside}"
    );
    let folded = &skills[skills.find(r#"id="built-in""#).unwrap()..];
    assert!(
        folded.contains(">same<") && folded.contains(">tool<"),
        "{folded}"
    );
    let tidy = get(&hub, "/skills/tidy").await;
    assert!(tidy.contains(head), "{tidy}");
}

#[tokio::test]
async fn a_clash_with_an_identical_skill_copies_nothing() {
    let hub = hub();
    let before = commits(&hub.knowledge);
    let (status, location, _) = post(
        &hub,
        "/import/skill",
        &[("key", "skill:~/.claude/skills/same")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(
        location.contains("done=already") && location.contains("unchanged=1"),
        "{location}"
    );
    assert_eq!(commits(&hub.knowledge), before);
}

#[tokio::test]
async fn a_different_clash_is_compared_and_theirs_can_replace_atlas_s() {
    let hub = hub();
    let key = "skill:~/.claude/skills/diff";
    let before = commits(&hub.knowledge);
    let (status, location, _) = post(&hub, "/import/skill", &[("key", key)]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.starts_with("/import/compare?key="), "{location}");
    assert_eq!(
        commits(&hub.knowledge),
        before,
        "nothing changes before a choice"
    );
    let page = get(&hub, &location).await;
    assert!(page.contains("Atlas's copy"));
    assert!(
        page.contains("Theirs") && page.contains("Differs"),
        "{page}"
    );
    assert!(page.contains("Keep Atlas"));
    let (status, _, body) = post(&hub, "/import/skill/theirs", &[("key", key)]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        fs::read_to_string(hub.knowledge.join("skills/diff/SKILL.md")).unwrap(),
        "---\ndescription: d\n---\nTheirs\n"
    );
    assert_eq!(commits(&hub.knowledge), before + 1);
}

#[tokio::test]
async fn claude_s_notes_come_in_byte_for_byte_and_identical_ones_are_skipped() {
    let hub = hub();
    let page = get(&hub, "/memory?scope=shop").await;
    assert!(page.contains("new.md") && page.contains("Atlas's copy differs"));
    let before = commits(&hub.knowledge);
    let (status, location, body) = post(&hub, "/import/notes", &[("scope", "shop")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(
        location.starts_with("/memory?scope=shop&done=notes"),
        "{location}"
    );
    let k = &hub.knowledge;
    assert_eq!(commits(k), before + 1);
    let store = k.join("memory/projects/shop");
    assert_eq!(
        fs::read(store.join("new.md")).unwrap(),
        fs::read(atlas_harness_memory(&hub.home, &hub.shop).join("new.md")).unwrap()
    );
    let index = fs::read_to_string(store.join("index.md")).unwrap();
    assert!(
        index.contains("- [new.md](new.md) · from test-hub"),
        "{index}"
    );
    assert!(!index.contains("same.md") && !index.contains("differs.md"));
    assert_eq!(
        fs::read_to_string(store.join("differs.md")).unwrap(),
        "Atlas's.\n"
    );
    let changed = git(k, &["show", "--name-only", "--format=", "HEAD"]);
    let mut changed: Vec<&str> = changed.lines().collect();
    changed.sort_unstable();
    assert_eq!(
        changed,
        [
            "memory/projects/shop/index.md",
            "memory/projects/shop/new.md"
        ]
    );

    // Nothing new is left, so a second Bring all in commits nothing.
    let (_, location, _) = post(&hub, "/import/notes", &[("scope", "shop")]).await;
    assert!(location.contains("unchanged=1"), "{location}");

    // A note Atlas has with other text needs the owner's word.
    let page = get(&hub, "/memory?scope=shop").await;
    let key = key_after(&page, "differs.md");
    let (status, _, body) = post(&hub, "/import/note", &[("key", &key)]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, _, _) = post(&hub, "/import/note", &[("key", &key), ("replace", "1")]).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert_eq!(
        fs::read_to_string(store.join("differs.md")).unwrap(),
        "This machine's.\n"
    );
}

#[tokio::test]
async fn a_hook_becomes_a_valid_hook_toml_and_script() {
    let hub = hub();
    let page = get(&hub, "/hooks").await;
    assert!(
        page.contains("API_KEY=••••") && !page.contains("abc123"),
        "masked"
    );
    let key = key_after(&page, "no-worktrees.py");
    let (status, location, body) = post(
        &hub,
        "/import/hook",
        &[("key", &key), ("name", "no-worktrees")],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(location.starts_with("/hooks?done=hook"), "{location}");
    let k = &hub.knowledge;
    let script = k.join("hooks/no-worktrees/run.sh");
    let text = fs::read_to_string(&script).unwrap();
    assert!(
        text.starts_with("#!/bin/sh\n")
            && text.ends_with("python3 ~/.claude/hooks/no-worktrees.py\n")
    );
    assert!(fs::metadata(&script).unwrap().permissions().mode() & 0o111 != 0);
    let sources = Sources {
        home: hub.home.clone(),
        knowledge: k.clone(),
        public_skills: None,
        projects: Vec::new(),
    };
    let catalog = atlas_fs::load_catalog(&sources);
    let hook = catalog
        .hooks
        .iter()
        .find(|h| h.name.as_str() == "no-worktrees")
        .unwrap_or_else(|| panic!("{:?}", catalog.problems));
    assert!(hook.script_present);
    assert_eq!(hook.manifest.event, HookEvent::PreToolUse);
    assert_eq!(hook.manifest.matcher.as_deref(), Some("Bash"));
    assert_eq!(hook.manifest.timeout, Some(10));
    assert_eq!(hook.manifest.harnesses, Some(vec![HarnessKind::ClaudeCode]));
    assert_eq!(
        hook.manifest.replaces.as_deref(),
        Some("python3 ~/.claude/hooks/no-worktrees.py")
    );
    assert!(catalog.problems.is_empty(), "{:?}", catalog.problems);

    // A command holding a secret never goes in git.
    let page = get(&hub, "/hooks").await;
    let key = key_after(&page, "notify.sh");
    let (status, _, body) = post(&hub, "/import/hook", &[("key", &key)]).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("secret"));
}

#[tokio::test]
async fn left_outside_is_recorded_and_not_offered_again() {
    let hub = hub();
    let key = "skill:~/.claude/skills/own";
    let (status, location, body) = post(&hub, "/import/keep", &[("key", key)]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(location.starts_with("/skills?done=kept"), "{location}");
    let placements = fs::read_to_string(hub.knowledge.join("atlas/placements.toml")).unwrap();
    assert!(
        placements.contains("skill:~/.claude/skills/own"),
        "{placements}"
    );
    assert!(
        !placements.contains(hub.home.to_str().unwrap()),
        "no absolute home in git"
    );
    let page = get(&hub, "/skills").await;
    assert!(!page.contains(r#"value="skill:~/.claude/skills/own""#));
    assert!(page.contains("1 more you chose to leave outside"));
}

#[tokio::test]
async fn changes_are_refused_when_the_store_is_dirty_or_the_item_is_unknown() {
    let hub = hub();
    write(&hub.knowledge.join("atlas/placements.toml"), "# by hand\n");
    let before = commits(&hub.knowledge);
    let (status, _, body) = post(
        &hub,
        "/import/keep",
        &[("key", "skill:~/.claude/skills/own")],
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(body.contains("uncommitted"), "{body}");
    assert_eq!(
        fs::read_to_string(hub.knowledge.join("atlas/placements.toml")).unwrap(),
        "# by hand\n"
    );
    let (status, _, _) = post(&hub, "/import/skill", &[("key", "skill:~/.ssh")]).await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _, _) = post_as(
        &hub,
        "/import/skill",
        &[("key", "skill:~/.claude/skills/own")],
        false,
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(commits(&hub.knowledge), before);
    assert!(!hub.knowledge.join("skills/own").exists());
}

#[tokio::test]
async fn tidy_up_commits_once_per_repo() {
    let hub = hub();
    let page = get(&hub, "/skills/tidy").await;
    assert!(
        page.contains("Move claude-only to .agents/skills"),
        "{page}"
    );
    let k_before = commits(&hub.knowledge);
    let s_before = commits(&hub.shop);
    let (status, location, body) = post(
        &hub,
        "/skills/tidy",
        &[
            ("r0", "bring:skill:~/.claude/skills/own"),
            ("share", "shop/claude-only"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(location.starts_with("/skills/tidy?saved="), "{location}");
    assert_eq!(commits(&hub.knowledge), k_before + 1);
    assert_eq!(commits(&hub.shop), s_before + 1);
    assert!(hub.knowledge.join("skills/own/SKILL.md").exists());
    assert!(
        hub.shop
            .join(".agents/skills/claude-only/SKILL.md")
            .exists()
    );
    assert!(!hub.shop.join(".claude/skills/claude-only").exists());
    assert_eq!(
        last_message(&hub.shop),
        "Tidy up shop: move 1 skill to .agents/skills"
    );
    // A stale choice changes nothing.
    let (status, _, _) = post(&hub, "/skills/tidy", &[("share", "shop/claude-only")]).await;
    assert_eq!(status, StatusCode::CONFLICT);
}
