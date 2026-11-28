//! The Instructions page's changes, over HTTP against a signed-in test hub with real git
//! repos: Use Global instead, CLAUDE.md to AGENTS.md (move and merge), Move to Global, Copy
//! to this project, a new CLAUDE.local.md, Claude's AGENTS.md option, and the refusals.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_core::kernel::Slug;
use atlas_fs::{Project, Sources};
use atlas_web::{Access, AppState, Writer, router};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

const ORIGIN: &str = "https://hub.test";
const LOGIN: &str = "owner@github";

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

fn repo(dir: &Path, files: &[(&str, &str)]) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "user.name", "Test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    for (rel, text) in files {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "start"]);
}

struct Hub {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    knowledge: PathBuf,
    project: PathBuf,
    app: axum::Router,
    cookie: String,
    csrf: String,
}

const GLOBAL: &str = "# Global\n\nBe brief.\n";

fn hub(project_files: &[(&str, &str)]) -> Hub {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let knowledge = home.join("Knowledge");
    repo(&knowledge, &[("AGENTS.md", GLOBAL)]);
    let project = home.join("Projects/shop");
    repo(&project, project_files);
    // Agents on the hub's machine: Codex with its own file, Claude importing Global.
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(home.join(".codex/AGENTS.md"), "Use rg.\n").unwrap();
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::write(
        home.join(".claude/CLAUDE.md"),
        format!("@{}\n", knowledge.join("AGENTS.md").display()),
    )
    .unwrap();
    let sources = Sources {
        home: home.clone(),
        knowledge: knowledge.clone(),
        public_skills: None,
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: project.clone(),
        }],
    };
    let writer = Writer::open(ORIGIN, &home.join("state"), std::time::Duration::ZERO).unwrap();
    let (cookie, csrf) = writer.test_session();
    let app = router(
        AppState::new(sources, "test-hub", Vec::new(), Some(writer)),
        Access::new(vec![LOGIN.to_owned()]),
    );
    Hub {
        _tmp: tmp,
        home,
        knowledge,
        project,
        app,
        cookie,
        csrf,
    }
}

struct Res {
    status: StatusCode,
    location: String,
    body: String,
}

async fn send(hub: &Hub, req: Request<Body>) -> Res {
    let res = hub.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let location = res
        .headers()
        .get(header::LOCATION)
        .map_or_else(String::new, |v| v.to_str().unwrap().to_owned());
    let body = to_bytes(res.into_body(), 1 << 22).await.unwrap();
    Res {
        status,
        location,
        // Askama escapes apostrophes; the assertions read the text as shown.
        body: String::from_utf8_lossy(&body).replace("&#39;", "'"),
    }
}

async fn get(hub: &Hub, path: &str) -> Res {
    let req = Request::get(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::COOKIE, &hub.cookie)
        .body(Body::empty())
        .unwrap();
    send(hub, req).await
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

async fn post(hub: &Hub, path: &str, fields: &[(&str, &str)], signed_in: bool) -> Res {
    let body = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let mut req = Request::post(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if signed_in {
        req = req.header(header::COOKIE, &hub.cookie);
    }
    send(hub, req.body(Body::from(body)).unwrap()).await
}

fn base_of(page: &str) -> String {
    let key = r#"name="base" value=""#;
    let at = page.find(key).expect("a base field") + key.len();
    page[at..at + 32].to_owned()
}

/// Opens the confirmation and posts it as shown.
async fn confirm(hub: &Hub, path: &str, extra: &[(&str, &str)]) -> Res {
    let page = get(hub, path).await;
    assert_eq!(page.status, StatusCode::OK, "{path}: {}", page.body);
    let base = base_of(&page.body);
    let mut fields = vec![("csrf", hub.csrf.as_str()), ("base", base.as_str())];
    fields.extend_from_slice(extra);
    post(hub, path, &fields, true).await
}

fn read(p: &Path) -> String {
    fs::read_to_string(p).unwrap()
}

fn commits(dir: &Path) -> usize {
    git(dir, &["rev-list", "--count", "HEAD"])
        .trim()
        .parse()
        .unwrap()
}

fn assert_committed(res: &Res) {
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    assert!(res.location.contains("saved="), "{}", res.location);
}

#[tokio::test]
async fn pages_show_who_reads_each_file() {
    let h = hub(&[("CLAUDE.md", "# Shop\n"), ("AGENTS.md", "# Shop agents\n")]);
    let list = get(&h, "/instructions").await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("Agents' own files"), "{}", list.body);
    assert!(list.body.contains("/instructions/global/codex/use-global"));
    assert!(
        list.body.contains("Not read by Codex"),
        "Codex has its own file: {}",
        list.body
    );
    assert!(list.body.contains("Claude reads a project"));
    assert!(!list.body.contains(" style=\""));

    let project = get(&h, "/instructions?scope=shop").await;
    assert!(
        project
            .body
            .contains("/instructions/shop/AGENTS.md/from-global")
    );
    assert!(project.body.contains("New CLAUDE.local.md"));

    let detail = get(&h, "/instructions/shop/AGENTS.md").await;
    assert_eq!(detail.status, StatusCode::OK);
    assert!(
        detail.body.contains("skips it while CLAUDE.md is there"),
        "Claude's default skips AGENTS.md: {}",
        detail.body
    );
    assert!(detail.body.contains("Move to Global"));
    let claude = get(&h, "/instructions/shop/CLAUDE.md").await;
    assert!(claude.body.contains("Merge into AGENTS.md"));
    assert!(
        claude.body.contains("/instructions/shop/CLAUDE.md/edit"),
        "the editor stays"
    );
    let global = get(&h, "/instructions/global/AGENTS.md").await;
    assert!(
        global
            .body
            .contains("reads it through ~/.claude/CLAUDE.md's import"),
        "{}",
        global.body
    );
    assert!(
        global.body.contains("uses its own ~/.codex/AGENTS.md"),
        "{}",
        global.body
    );
    // The editor route still wins over the change routes.
    assert_eq!(
        get(&h, "/instructions/global/AGENTS.md/edit").await.status,
        StatusCode::OK
    );
    assert_eq!(
        get(&h, "/instructions/shop/README.md").await.status,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn use_global_instead_adds_the_agents_text_to_global() {
    let h = hub(&[("AGENTS.md", "# Shop\n")]);
    let page = get(&h, "/instructions/global/codex/use-global").await;
    assert!(
        page.body.contains("Use rg."),
        "the text is shown: {}",
        page.body
    );
    assert!(page.body.contains("git can undo it"));
    let before = commits(&h.knowledge);
    let res = confirm(&h, "/instructions/global/codex/use-global", &[]).await;
    assert_committed(&res);
    assert_eq!(
        read(&h.knowledge.join("AGENTS.md")),
        format!("{GLOBAL}\n<!-- atlas: from ~/.codex/AGENTS.md -->\n\nUse rg.\n")
    );
    assert_eq!(commits(&h.knowledge), before + 1);
    // The hub leaves the machine's file to atlasd.
    assert_eq!(read(&h.home.join(".codex/AGENTS.md")), "Use rg.\n");
    // Again: Global holds it all now, so nothing more is added.
    let again = confirm(&h, "/instructions/global/codex/use-global", &[]).await;
    assert!(again.location.contains("unchanged=1"), "{}", again.location);
}

#[tokio::test]
async fn claude_md_becomes_agents_md() {
    let h = hub(&[("CLAUDE.md", "# Shop\nShip on Fridays.\n")]);
    let page = get(&h, "/instructions/shop/CLAUDE.md/to-agents").await;
    assert!(page.body.contains("Make it AGENTS.md"));
    assert!(
        !page.body.contains("Also let Claude"),
        "Claude reads AGENTS.md without CLAUDE.md"
    );
    let res = confirm(&h, "/instructions/shop/CLAUDE.md/to-agents", &[]).await;
    assert_committed(&res);
    assert_eq!(
        read(&h.project.join("AGENTS.md")),
        "# Shop\nShip on Fridays.\n"
    );
    assert!(!h.project.join("CLAUDE.md").exists());
    assert_eq!(commits(&h.project), 2);
    assert_eq!(git(&h.project, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn claude_md_merges_into_agents_md_and_can_turn_claudes_option_on() {
    let h = hub(&[
        ("AGENTS.md", "# Shop\n"),
        ("CLAUDE.md", "@AGENTS.md\nRun the tests.\n"),
        (".gitignore", "CLAUDE.local.md\n"),
    ]);
    fs::write(h.project.join("CLAUDE.local.md"), "Mine.\n").unwrap();
    let page = get(&h, "/instructions/shop/CLAUDE.md/to-agents").await;
    assert!(
        page.body.contains("while CLAUDE.local.md is there"),
        "Claude would still skip AGENTS.md: {}",
        page.body
    );
    let res = confirm(
        &h,
        "/instructions/shop/CLAUDE.md/to-agents",
        &[("claude", "1")],
    )
    .await;
    assert_committed(&res);
    assert_eq!(
        read(&h.project.join("AGENTS.md")),
        "# Shop\n\n<!-- atlas: from CLAUDE.md -->\n\nRun the tests.\n",
        "the import of AGENTS.md itself isn't carried over"
    );
    assert!(!h.project.join("CLAUDE.md").exists());
    assert!(
        read(&h.knowledge.join("atlas/placements.toml"))
            .contains("claude-reads-agents-md = \"alongside\""),
    );
    assert_eq!(git(&h.knowledge, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn move_to_global_and_copy_to_this_project() {
    let h = hub(&[("AGENTS.md", "# Shop\nUse pnpm.\n")]);
    let res = confirm(&h, "/instructions/shop/AGENTS.md/to-global", &[]).await;
    assert_committed(&res);
    assert_eq!(
        read(&h.knowledge.join("AGENTS.md")),
        format!("{GLOBAL}\n<!-- atlas: from shop/AGENTS.md -->\n\n# Shop\nUse pnpm.\n")
    );
    assert!(!h.project.join("AGENTS.md").exists());
    assert_eq!((commits(&h.knowledge), commits(&h.project)), (2, 2));

    // Copy: Global's text into the project's (now missing) AGENTS.md; Global stays.
    let global = read(&h.knowledge.join("AGENTS.md"));
    let res = confirm(&h, "/instructions/shop/AGENTS.md/from-global", &[]).await;
    assert_committed(&res);
    assert_eq!(
        read(&h.project.join("AGENTS.md")),
        format!("<!-- atlas: from Global AGENTS.md -->\n\n{global}")
    );
    assert_eq!(read(&h.knowledge.join("AGENTS.md")), global);
}

#[tokio::test]
async fn a_new_claude_local_md_is_kept_out_of_git() {
    let h = hub(&[("AGENTS.md", "# Shop\n")]);
    fs::write(h.project.join(".git/info/exclude"), "*.swp\n").unwrap();
    let page = get(&h, "/instructions/shop/CLAUDE.local.md/create").await;
    assert!(page.body.contains("git can't undo it"), "{}", page.body);
    let res = post(
        &h,
        "/instructions/shop/CLAUDE.local.md/create",
        &[("csrf", &h.csrf), ("text", "Only mine.\r\n")],
        true,
    )
    .await;
    assert_eq!(res.status, StatusCode::SEE_OTHER, "{}", res.body);
    assert!(res.location.ends_with("?local=1"), "{}", res.location);
    assert_eq!(read(&h.project.join("CLAUDE.local.md")), "Only mine.\n");
    assert_eq!(
        read(&h.project.join(".git/info/exclude")),
        "*.swp\n# atlas:begin\n/CLAUDE.local.md\n# atlas:end\n"
    );
    assert_eq!(git(&h.project, &["status", "--porcelain"]), "");
    assert_eq!(commits(&h.project), 1);
    let detail = get(&h, &res.location).await;
    assert!(detail.body.contains("kept out of git"), "{}", detail.body);
    assert!(detail.body.contains("Only mine."));
    assert!(
        !detail.body.contains("CLAUDE.local.md/edit"),
        "not in git, so no editor"
    );

    // Moving it to Global: Global's commit first, then the file goes.
    let res = confirm(&h, "/instructions/shop/CLAUDE.local.md/to-global", &[]).await;
    assert_committed(&res);
    assert!(read(&h.knowledge.join("AGENTS.md")).contains("Only mine."));
    assert!(!h.project.join("CLAUDE.local.md").exists());

    // A new file that already exists is refused.
    let res = post(
        &h,
        "/instructions/shop/AGENTS.md/create",
        &[("csrf", &h.csrf), ("text", "x")],
        true,
    )
    .await;
    assert_eq!(res.status, StatusCode::BAD_REQUEST, "{}", res.body);
}

#[tokio::test]
async fn claudes_option_is_stored_for_every_machine() {
    let h = hub(&[("AGENTS.md", "# Shop\n")]);
    let res = post(
        &h,
        "/instructions/claude-agents-md",
        &[("csrf", &h.csrf), ("value", "never")],
        true,
    )
    .await;
    assert_committed(&res);
    assert!(
        read(&h.knowledge.join("atlas/placements.toml"))
            .contains("claude-reads-agents-md = \"never\"")
    );
    let list = get(&h, "/instructions").await;
    assert!(
        list.body.contains(r#"<option value="never" selected>"#),
        "{}",
        list.body
    );
    let bad = post(
        &h,
        "/instructions/claude-agents-md",
        &[("csrf", &h.csrf), ("value", "sometimes")],
        true,
    )
    .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn refusals_change_nothing() {
    let h = hub(&[("AGENTS.md", "# Shop\n"), ("CLAUDE.md", "Rules.\n")]);
    let path = "/instructions/shop/CLAUDE.md/to-agents";

    // No session, or a stale CSRF token.
    let page = get(&h, path).await;
    let base = base_of(&page.body);
    let res = post(&h, path, &[("csrf", &h.csrf), ("base", &base)], false).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);
    let res = post(&h, path, &[("csrf", "nope"), ("base", &base)], true).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN);

    // The file changed since the confirmation was shown.
    fs::write(h.project.join("CLAUDE.md"), "Rules.\nMore.\n").unwrap();
    let res = post(&h, path, &[("csrf", &h.csrf), ("base", &base)], true).await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert!(
        res.body.contains("changed since you opened"),
        "{}",
        res.body
    );

    // Uncommitted changes made outside Atlas.
    let res = confirm(&h, path, &[]).await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert!(res.body.contains("uncommitted changes"), "{}", res.body);
    git(&h.project, &["checkout", "--", "CLAUDE.md"]);

    // Not on main.
    git(&h.project, &["checkout", "-q", "-b", "feature"]);
    let res = confirm(&h, path, &[]).await;
    assert_eq!(res.status, StatusCode::CONFLICT);
    assert!(res.body.contains("not main"), "{}", res.body);
    git(&h.project, &["checkout", "-q", "main"]);

    // Unknown names never reach a write.
    for p in [
        "/instructions/shop/README.md/to-global",
        "/instructions/shop/AGENTS.md/to-agents",
        "/instructions/global/t3/use-global",
        "/instructions/nowhere/AGENTS.md/create",
    ] {
        let res = post(&h, p, &[("csrf", &h.csrf)], true).await;
        assert_eq!(res.status, StatusCode::NOT_FOUND, "{p}");
    }
    assert_eq!(read(&h.project.join("CLAUDE.md")), "Rules.\n");
    assert!(h.project.join("AGENTS.md").exists());
    assert_eq!(commits(&h.project), 1);
    assert_eq!(commits(&h.knowledge), 1);
}

/// atlasd's copy of Global on a hub machine that also runs atlasd.
const ATLASD_GLOBAL: &str = ".local/share/atlas/sources/knowledge/AGENTS.md";

#[tokio::test]
async fn use_global_instead_drops_atlasds_import_too() {
    let h = hub(&[("AGENTS.md", "# Shop\n")]);
    let copy = h.home.join(ATLASD_GLOBAL);
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::write(&copy, GLOBAL).unwrap();
    fs::write(
        h.home.join(".claude/CLAUDE.md"),
        format!("@~/{ATLASD_GLOBAL}\nPrefer rg.\n"),
    )
    .unwrap();
    let res = confirm(&h, "/instructions/global/claude-code/use-global", &[]).await;
    assert_committed(&res);
    assert_eq!(
        read(&h.knowledge.join("AGENTS.md")),
        format!("{GLOBAL}\n<!-- atlas: from ~/.claude/CLAUDE.md -->\n\nPrefer rg.\n"),
        "Global never gains an import of itself"
    );
}

#[tokio::test]
async fn move_to_global_drops_every_import_of_global() {
    // Imports of Global by atlasd's copy and through a link to the knowledge repo.
    let h = hub(&[("README.md", "Shop.\n")]);
    let copy = h.home.join(ATLASD_GLOBAL);
    fs::create_dir_all(copy.parent().unwrap()).unwrap();
    fs::write(&copy, GLOBAL).unwrap();
    std::os::unix::fs::symlink(&h.knowledge, h.home.join("K")).unwrap();
    fs::write(
        h.project.join("AGENTS.md"),
        format!("@~/{ATLASD_GLOBAL}\n@~/K/AGENTS.md\nUse pnpm.\n"),
    )
    .unwrap();
    git(&h.project, &["add", "-A"]);
    git(&h.project, &["commit", "-q", "-m", "agents"]);
    let res = confirm(&h, "/instructions/shop/AGENTS.md/to-global", &[]).await;
    assert_committed(&res);
    assert_eq!(
        read(&h.knowledge.join("AGENTS.md")),
        format!("{GLOBAL}\n<!-- atlas: from shop/AGENTS.md -->\n\nUse pnpm.\n")
    );
}

#[tokio::test]
async fn moving_a_claude_local_md_that_imports_global_removes_it() {
    let h = hub(&[("AGENTS.md", "# Shop\n")]);
    fs::write(h.project.join(".git/info/exclude"), "/CLAUDE.local.md\n").unwrap();
    fs::write(
        h.project.join("CLAUDE.local.md"),
        "@~/Knowledge/AGENTS.md\nOnly mine.\n",
    )
    .unwrap();
    let res = confirm(&h, "/instructions/shop/CLAUDE.local.md/to-global", &[]).await;
    assert_committed(&res);
    assert!(!res.location.contains("kept=1"), "{}", res.location);
    assert!(!h.project.join("CLAUDE.local.md").exists());
    assert_eq!(
        read(&h.knowledge.join("AGENTS.md")),
        format!("{GLOBAL}\n<!-- atlas: from shop/CLAUDE.local.md -->\n\nOnly mine.\n")
    );
    // The next page reads the project as it now is.
    let list = get(&h, "/instructions?scope=shop").await;
    assert!(
        !list
            .body
            .contains(r#"href="/instructions/shop/CLAUDE.local.md""#),
        "{}",
        list.body
    );
    assert!(list.body.contains("New CLAUDE.local.md"));
}

#[tokio::test]
async fn nothing_is_read_or_planned_before_the_session_is_checked() {
    let h = hub(&[("AGENTS.md", "# Shop\n"), ("CLAUDE.md", "Rules.\n")]);
    let path = "/instructions/shop/CLAUDE.md/to-agents";
    // A stale base used to answer 409, and a plan that fails 400, before the session
    // check: both mean the files were read and git ran for a stranger.
    let res = post(&h, path, &[("csrf", &h.csrf), ("base", "stale")], false).await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    let res = post(
        &h,
        "/instructions/shop/CLAUDE.local.md/to-global",
        &[("csrf", &h.csrf)],
        false,
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    // Cross-origin, with the session cookie.
    let req = Request::post(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::ORIGIN, "https://elsewhere.test")
        .header(header::COOKIE, &h.cookie)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("csrf={}&base=stale", h.csrf)))
        .unwrap();
    assert_eq!(send(&h, req).await.status, StatusCode::FORBIDDEN);
    // Claude's option: an unknown value is refused only after the session check.
    let res = post(
        &h,
        "/instructions/claude-agents-md",
        &[("csrf", "nope"), ("value", "sometimes")],
        true,
    )
    .await;
    assert_eq!(res.status, StatusCode::FORBIDDEN, "{}", res.body);
    assert_eq!(read(&h.project.join("CLAUDE.md")), "Rules.\n");
    assert_eq!((commits(&h.project), commits(&h.knowledge)), (1, 1));
}
