//! The skill library over HTTP: real temporary git repos and a signed-in hub.

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_core::kernel::Slug;
use atlas_fs::{Project, Sources};
use atlas_web::{Access, AppState, Writer, router};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use tower::ServiceExt;

mod common;

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
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
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
    if dir.join("skills/pdf/scripts/run.sh").exists() {
        fs::set_permissions(
            dir.join("skills/pdf/scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "start"]);
}

fn commits(dir: &Path) -> usize {
    git(dir, &["rev-list", "--count", "HEAD"]).parse().unwrap()
}

struct Hub {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    knowledge: PathBuf,
    shop: PathBuf,
    global_lock: PathBuf,
    app: axum::Router,
    cookie: String,
    csrf: String,
}

const SHOP_LOCK: &str = r#"{
  "version": 1,
  "skills": {
    "stock": {
      "source": "acme/shop-tools",
      "sourceType": "github",
      "computedHash": "0000000000000000000000000000000000000000000000000000000000000000"
    }
  }
}
"#;

fn hub() -> Hub {
    hub_with(false)
}

/// `applied`: the hub's own machine has applied an update with everything saved.
fn hub_with(applied: bool) -> Hub {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let knowledge = home.join("Knowledge");
    repo(
        &knowledge,
        &[
            ("AGENTS.md", "# Rules\n"),
            (
                "skills/pdf/SKILL.md",
                "---\nname: pdf\ndescription: Reads PDFs\n---\n# PDF\n",
            ),
            ("skills/pdf/scripts/run.sh", "echo <b>hi</b>\n"),
            (
                "skills/pdf/references/notes.md",
                "# Notes\n<script>alert(1)</script>\n",
            ),
            (
                "skills/keep/SKILL.md",
                "---\ndescription: Keeps things\n---\nBody\n",
            ),
            ("skills/a/SKILL.md", "---\ndescription: A\n---\n"),
            ("skills/b/SKILL.md", "---\ndescription: B\n---\n"),
            ("skills/c/SKILL.md", "---\ndescription: C\n---\n"),
            ("atlas/placements.toml", "[global.pdf]\noff = [\"codex\"]\n"),
        ],
    );
    // The skills CLI's global lock, kept inside the knowledge repo for this hub, with a
    // tracked record for pdf (its tree SHA as GitHub would report it).
    let tree = git(&knowledge, &["rev-parse", "HEAD:skills/pdf"]);
    let global_lock = knowledge.join("agents/.skill-lock.json");
    write(
        &global_lock,
        &format!(
            "{{\n  \"version\": 3,\n  \"skills\": {{\n    \"pdf\": {{\n      \"source\": \"acme/tools\",\n      \"sourceType\": \"github\",\n      \"sourceUrl\": \"https://github.com/acme/tools.git\",\n      \"skillPath\": \"skills/pdf/SKILL.md\",\n      \"skillFolderHash\": \"{tree}\",\n      \"installedAt\": \"2026-01-01T00:00:00.000Z\",\n      \"updatedAt\": \"2026-01-01T00:00:00.000Z\"\n    }}\n  }},\n  \"dismissed\": {{}}\n}}"
        ),
    );
    git(&knowledge, &["add", "-A"]);
    git(&knowledge, &["commit", "-q", "-m", "lock"]);
    let shop = home.join("Projects/shop");
    repo(
        &shop,
        &[
            ("AGENTS.md", "# Shop\n"),
            (
                ".agents/skills/stock/SKILL.md",
                "---\ndescription: Count stock\n---\nBody\n",
            ),
            ("skills-lock.json", SHOP_LOCK),
        ],
    );
    let sources = Sources {
        home: home.clone(),
        knowledge: knowledge.clone(),
        public_skills: None,
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: shop.clone(),
        }],
    };
    if applied {
        common::applied_update(&home.join("state"), &knowledge, ORIGIN, "test-hub");
    }
    let writer = Writer::open(
        ORIGIN,
        &home.join("state"),
        std::time::Duration::from_mins(30),
    )
    .unwrap();
    let cookie = writer.auth.sign_in();
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        format!("{COOKIE}={cookie}").parse().unwrap(),
    );
    let csrf = writer.auth.session(&h).unwrap().csrf;
    let state = AppState::new(sources, "test-hub", Vec::new(), Some(writer))
        .with_github_stand_in("http://127.0.0.1:9", global_lock.clone());
    let app = router(state, Access::new(vec![LOGIN.to_owned()]));
    Hub {
        _tmp: tmp,
        home,
        knowledge,
        shop,
        global_lock,
        app,
        cookie,
        csrf,
    }
}

struct Got {
    status: StatusCode,
    location: String,
    body: String,
}

async fn send(hub: &Hub, req: Request<Body>) -> Got {
    let res = hub.app.clone().oneshot(req).await.unwrap();
    let status = res.status();
    let location = res
        .headers()
        .get(header::LOCATION)
        .map_or_else(String::new, |v| v.to_str().unwrap().to_owned());
    let body = to_bytes(res.into_body(), 1 << 22).await.unwrap();
    Got {
        status,
        location,
        body: String::from_utf8_lossy(&body).into_owned(),
    }
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn get(hub: &Hub, path: &str) -> Got {
    let req = Request::get(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::COOKIE, format!("{COOKIE}={}", hub.cookie))
        .body(Body::empty())
        .unwrap();
    send(hub, req).await
}

async fn post(hub: &Hub, path: &str, fields: &[(&str, &str)]) -> Got {
    post_with(hub, path, fields, &hub.csrf, false).await
}

async fn post_with(hub: &Hub, path: &str, fields: &[(&str, &str)], csrf: &str, json: bool) -> Got {
    let mut body: Vec<String> = vec![format!("csrf={}", enc(csrf))];
    body.extend(fields.iter().map(|(k, v)| format!("{k}={}", enc(v))));
    let mut req = Request::post(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, format!("{COOKIE}={}", hub.cookie))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if json {
        req = req.header(header::ACCEPT, "application/json");
    }
    send(hub, req.body(Body::from(body.join("&"))).unwrap()).await
}

fn placements(hub: &Hub) -> String {
    fs::read_to_string(hub.knowledge.join("atlas/placements.toml")).unwrap_or_default()
}

#[tokio::test]
async fn a_new_skill_is_written_committed_and_opened() {
    let hub = hub();
    let before = commits(&hub.knowledge);
    let page = get(&hub, "/skills/new").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("name=\"description\""));
    let got = post(
        &hub,
        "/skills/new",
        &[
            ("name", "tidy-notes"),
            ("description", "Tidies notes: \"fast\""),
            ("where", "global"),
        ],
    )
    .await;
    assert_eq!(got.status, StatusCode::SEE_OTHER, "{}", got.body);
    assert!(
        got.location
            .starts_with("/skills/private/tidy-notes?saved="),
        "{}",
        got.location
    );
    let text = fs::read_to_string(hub.knowledge.join("skills/tidy-notes/SKILL.md")).unwrap();
    assert!(
        text.starts_with(
            "---\nname: tidy-notes\ndescription: \"Tidies notes: \\\"fast\\\"\"\n---\n"
        )
    );
    assert_eq!(commits(&hub.knowledge), before + 1);
    assert_eq!(git(&hub.knowledge, &["status", "--porcelain"]), "");
    let opened = get(&hub, "/skills/private/tidy-notes").await;
    assert_eq!(opened.status, StatusCode::OK);
    assert!(opened.body.contains("Tidies notes"));

    // In this project.
    let got = post(
        &hub,
        "/skills/new",
        &[
            ("name", "ship"),
            ("description", "Ships"),
            ("where", "shop"),
        ],
    )
    .await;
    assert_eq!(got.status, StatusCode::SEE_OTHER, "{}", got.body);
    assert!(hub.shop.join(".agents/skills/ship/SKILL.md").exists());
}

#[tokio::test]
async fn bad_or_taken_names_are_refused_and_nothing_changes() {
    let hub = hub();
    // A harness's own folder holds this name.
    write(
        &hub.home.join(".claude/skills/handmade/SKILL.md"),
        "---\ndescription: mine\n---\n",
    );
    let before = commits(&hub.knowledge);
    for (name, why) in [
        ("Bad Name", "lowercase letters"),
        ("../evil", "lowercase letters"),
        ("pdf", "already exists"),
        ("stock", "already exists"),
        ("handmade", "already exists"),
    ] {
        let got = post(
            &hub,
            "/skills/new",
            &[("name", name), ("description", "x"), ("where", "global")],
        )
        .await;
        assert_eq!(got.status, StatusCode::CONFLICT, "{name}");
        assert!(got.body.contains(why), "{name}: {}", got.body);
    }
    let empty = post(
        &hub,
        "/skills/new",
        &[("name", "fine"), ("description", "  "), ("where", "global")],
    )
    .await;
    assert_eq!(empty.status, StatusCode::CONFLICT);
    let json = post_with(
        &hub,
        "/skills/new",
        &[("name", "pdf"), ("description", "x"), ("where", "global")],
        &hub.csrf,
        true,
    )
    .await;
    assert_eq!(json.status, StatusCode::CONFLICT);
    assert!(json.body.starts_with("{\"error\":"), "{}", json.body);
    let stale = post_with(
        &hub,
        "/skills/new",
        &[("name", "fine"), ("description", "x"), ("where", "global")],
        "wrong",
        false,
    )
    .await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    assert_eq!(commits(&hub.knowledge), before);
    assert!(!hub.knowledge.join("skills/fine").exists());
}

#[tokio::test]
async fn deleting_asks_first_then_removes_folder_and_entry_in_one_commit() {
    let hub = hub();
    let before = commits(&hub.knowledge);
    let ask = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "delete"),
            ("s", "private/pdf"),
            ("back", "/skills"),
        ],
    )
    .await;
    assert_eq!(ask.status, StatusCode::SEE_OTHER);
    assert!(
        ask.location.starts_with("/skills/confirm?"),
        "{}",
        ask.location
    );
    assert!(!ask.location.contains("csrf"));
    let page = get(&hub, &ask.location).await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Delete 1 skill?"), "{}", page.body);
    assert!(page.body.contains("Knowledge/skills/pdf"));
    assert!(page.body.contains("git can undo"));
    assert!(
        hub.knowledge.join("skills/pdf").exists(),
        "asking changed nothing"
    );

    let done = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "delete"),
            ("s", "private/pdf"),
            ("back", "/skills"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.body);
    assert!(
        done.location.starts_with("/skills?saved="),
        "{}",
        done.location
    );
    assert!(!hub.knowledge.join("skills/pdf").exists());
    assert!(!placements(&hub).contains("pdf"), "{}", placements(&hub));
    let lock = fs::read_to_string(&hub.global_lock).unwrap();
    assert!(!lock.contains("\"pdf\""), "{lock}");
    assert_eq!(commits(&hub.knowledge), before + 1);
    assert_eq!(git(&hub.knowledge, &["status", "--porcelain"]), "");
    let changed = git(
        &hub.knowledge,
        &["show", "--name-only", "--format=%s", "HEAD"],
    );
    assert!(changed.starts_with("Delete pdf in Atlas"), "{changed}");
    assert!(changed.contains("atlas/placements.toml"));
    assert!(changed.contains("skills/pdf/SKILL.md"));
}

#[tokio::test]
async fn a_bulk_delete_is_one_commit() {
    let hub = hub();
    let before = commits(&hub.knowledge);
    let done = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "delete"),
            ("s", "private/a"),
            ("s", "private/b"),
            ("s", "private/c"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.body);
    for n in ["a", "b", "c"] {
        assert!(!hub.knowledge.join("skills").join(n).exists());
    }
    assert_eq!(commits(&hub.knowledge), before + 1);
}

#[tokio::test]
async fn moving_to_a_project_commits_both_and_carries_switches_and_lock_record() {
    let hub = hub();
    let (k0, s0) = (commits(&hub.knowledge), commits(&hub.shop));
    let page = get(
        &hub,
        "/skills/confirm?action=move&s=private%2Fpdf&to=shop&back=%2Fskills",
    )
    .await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(page.body.contains("Move 1 skill to shop?"), "{}", page.body);
    let done = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "move"),
            ("s", "private/pdf"),
            ("to", "shop"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.body);
    assert!(!hub.knowledge.join("skills/pdf").exists());
    assert!(hub.shop.join(".agents/skills/pdf/SKILL.md").exists());
    // Scripts stay runnable.
    let staged = git(
        &hub.shop,
        &["ls-files", "-s", ".agents/skills/pdf/scripts/run.sh"],
    );
    assert!(staged.starts_with("100755"), "{staged}");
    assert_eq!(commits(&hub.knowledge), k0 + 1);
    assert_eq!(commits(&hub.shop), s0 + 1);
    // Switches: the global entry became the project's.
    let p = placements(&hub);
    assert!(p.contains("[project.shop.pdf]"), "{p}");
    assert!(!p.contains("[global.pdf]"), "{p}");
    // The lock record moved, as the project lock keeps it.
    let global = fs::read_to_string(&hub.global_lock).unwrap();
    assert!(!global.contains("\"pdf\""), "{global}");
    let project = fs::read_to_string(hub.shop.join("skills-lock.json")).unwrap();
    assert!(project.contains("\"pdf\""), "{project}");
    assert!(project.contains("\"computedHash\""), "{project}");
    assert!(project.ends_with("}\n"));
    assert_eq!(git(&hub.knowledge, &["status", "--porcelain"]), "");
    assert_eq!(git(&hub.shop, &["status", "--porcelain"]), "");
}

#[tokio::test]
async fn moving_to_global_commits_both_and_carries_the_lock_record() {
    let hub = hub();
    let (k0, s0) = (commits(&hub.knowledge), commits(&hub.shop));
    let done = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "move"),
            ("s", "shop/stock"),
            ("to", "private"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.body);
    assert!(hub.knowledge.join("skills/stock/SKILL.md").exists());
    assert!(!hub.shop.join(".agents/skills/stock").exists());
    assert_eq!(commits(&hub.knowledge), k0 + 1);
    assert_eq!(commits(&hub.shop), s0 + 1);
    let project = fs::read_to_string(hub.shop.join("skills-lock.json")).unwrap();
    assert!(!project.contains("stock"), "{project}");
    let global = fs::read_to_string(&hub.global_lock).unwrap();
    assert!(global.contains("\"stock\""), "{global}");
    assert!(
        global.contains("https://github.com/acme/shop-tools.git"),
        "{global}"
    );
    assert!(
        !global.ends_with('\n'),
        "the global lock keeps its formatting"
    );
}

#[tokio::test]
async fn a_lock_outside_git_is_left_alone_and_the_confirmation_says_so() {
    let hub = hub();
    let outside = hub.home.join(".agents/.skill-lock.json");
    write(
        &outside,
        "{\n  \"version\": 3,\n  \"skills\": {\n    \"keep\": {\"source\": \"acme/x\", \"sourceType\": \"github\", \"sourceUrl\": \"https://github.com/acme/x.git\", \"skillFolderHash\": \"\"}\n  },\n  \"dismissed\": {}\n}",
    );
    let sources = Sources {
        home: hub.home.clone(),
        knowledge: hub.knowledge.clone(),
        public_skills: None,
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: hub.shop.clone(),
        }],
    };
    let writer = Writer::open(
        ORIGIN,
        &hub.home.join("state2"),
        std::time::Duration::from_mins(30),
    )
    .unwrap();
    let cookie = writer.auth.sign_in();
    let mut h = HeaderMap::new();
    h.insert(
        header::COOKIE,
        format!("{COOKIE}={cookie}").parse().unwrap(),
    );
    let csrf = writer.auth.session(&h).unwrap().csrf;
    let app = router(
        AppState::new(sources, "test-hub", Vec::new(), Some(writer))
            .with_github_stand_in("http://127.0.0.1:9", outside.clone()),
        Access::new(vec![LOGIN.to_owned()]),
    );
    let hub2 = Hub {
        _tmp: tempfile::tempdir().unwrap(),
        home: hub.home.clone(),
        knowledge: hub.knowledge.clone(),
        shop: hub.shop.clone(),
        global_lock: outside.clone(),
        app,
        cookie,
        csrf,
    };
    let page = get(
        &hub2,
        "/skills/confirm?action=move&s=private%2Fkeep&to=shop&back=%2Fskills",
    )
    .await;
    assert!(
        page.body
            .contains("isn&#39;t in a repo Atlas commits, so Atlas changes neither lock"),
        "{}",
        page.body
    );
    let list = get(&hub2, "/skills").await;
    assert!(
        list.body.contains("From acme/x"),
        "the list groups by source"
    );
    let before = fs::read_to_string(&outside).unwrap();
    let done = post(
        &hub2,
        "/skills/bulk",
        &[
            ("action", "move"),
            ("s", "private/keep"),
            ("to", "shop"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::SEE_OTHER, "{}", done.body);
    assert_eq!(fs::read_to_string(&outside).unwrap(), before);
    assert!(hub.shop.join(".agents/skills/keep").exists());
}

#[tokio::test]
async fn a_taken_destination_refuses_the_move() {
    let hub = hub();
    write(
        &hub.shop.join(".agents/skills/keep/SKILL.md"),
        "---\ndescription: theirs\n---\n",
    );
    git(&hub.shop, &["add", "-A"]);
    git(&hub.shop, &["commit", "-q", "-m", "theirs"]);
    let (k0, s0) = (commits(&hub.knowledge), commits(&hub.shop));
    let done = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "move"),
            ("s", "private/keep"),
            ("to", "shop"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::CONFLICT);
    assert!(done.body.contains("already exists"), "{}", done.body);
    assert!(hub.knowledge.join("skills/keep").exists());
    assert_eq!(
        fs::read_to_string(hub.shop.join(".agents/skills/keep/SKILL.md")).unwrap(),
        "---\ndescription: theirs\n---\n"
    );
    assert_eq!((commits(&hub.knowledge), commits(&hub.shop)), (k0, s0));
}

#[tokio::test]
async fn uncommitted_edits_refuse_the_change_and_nothing_changes() {
    let hub = hub();
    let edited = hub.knowledge.join("skills/keep/SKILL.md");
    fs::write(&edited, "my edit\n").unwrap();
    let before = commits(&hub.knowledge);
    let done = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "delete"),
            ("s", "private/keep"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(done.status, StatusCode::CONFLICT);
    assert!(done.body.contains("uncommitted"), "{}", done.body);
    assert_eq!(fs::read_to_string(&edited).unwrap(), "my edit\n");
    assert_eq!(commits(&hub.knowledge), before);
    let moved = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "move"),
            ("s", "private/keep"),
            ("to", "shop"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(moved.status, StatusCode::CONFLICT);
    assert!(!hub.shop.join(".agents/skills/keep").exists());
}

#[tokio::test]
async fn switching_many_asks_first_and_is_one_commit() {
    let hub = hub();
    let before = commits(&hub.knowledge);
    // Turning several off asks first.
    let ask = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "off"),
            ("s", "private/a"),
            ("s", "private/b"),
            ("agent", "codex"),
        ],
    )
    .await;
    assert_eq!(ask.status, StatusCode::SEE_OTHER);
    assert!(
        ask.location.starts_with("/skills/confirm?action=off"),
        "{}",
        ask.location
    );
    let page = get(&hub, &ask.location).await;
    assert!(
        page.body.contains("Turn 2 skills off for Codex?"),
        "{}",
        page.body
    );
    // Confirmed, the switch goes through set_agents as one commit.
    let off = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "off"),
            ("s", "private/a"),
            ("s", "private/b"),
            ("agent", "codex"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(off.status, StatusCode::SEE_OTHER, "{}", off.body);
    let p = placements(&hub);
    assert!(p.contains("[global.a]") && p.contains("[global.b]"), "{p}");
    assert_eq!(commits(&hub.knowledge), before + 1);
    // Turning on needs no confirmation and puts them back, again as one commit.
    let on = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "on"),
            ("s", "private/a"),
            ("s", "private/b"),
            ("agent", "codex"),
        ],
    )
    .await;
    assert_eq!(on.status, StatusCode::SEE_OTHER, "{}", on.body);
    assert!(
        !placements(&hub).contains("[global.a]"),
        "{}",
        placements(&hub)
    );
    assert_eq!(commits(&hub.knowledge), before + 2);
    let before = commits(&hub.knowledge);
    let bad_agent = post(
        &hub,
        "/skills/bulk",
        &[("action", "on"), ("s", "private/a"), ("agent", "t3")],
    )
    .await;
    assert_eq!(bad_agent.status, StatusCode::BAD_REQUEST);
    let stale = post_with(
        &hub,
        "/skills/bulk",
        &[("action", "on"), ("s", "private/a")],
        "wrong",
        false,
    )
    .await;
    assert_eq!(stale.status, StatusCode::FORBIDDEN);
    let unknown = post(
        &hub,
        "/skills/bulk",
        &[
            ("action", "delete"),
            ("s", "private/nope"),
            ("confirmed", "1"),
        ],
    )
    .await;
    assert_eq!(unknown.status, StatusCode::CONFLICT);
    assert_eq!(commits(&hub.knowledge), before);
}

#[tokio::test]
async fn the_page_script_gets_json_answers() {
    let hub = hub();
    let ask = post_with(
        &hub,
        "/skills/bulk",
        &[("action", "delete"), ("s", "private/a")],
        &hub.csrf,
        true,
    )
    .await;
    assert_eq!(ask.status, StatusCode::OK);
    assert!(
        ask.body.contains("\"next\":\"/skills/confirm?"),
        "{}",
        ask.body
    );
    let done = post_with(
        &hub,
        "/skills/bulk",
        &[("action", "delete"), ("s", "private/a"), ("confirmed", "1")],
        &hub.csrf,
        true,
    )
    .await;
    assert_eq!(done.status, StatusCode::OK);
    assert!(done.body.contains("\"send\":true"), "{}", done.body);
}

#[tokio::test]
async fn the_file_viewer_shows_only_listed_files_escaped() {
    let hub = hub();
    write(&hub.knowledge.join("skills/pdf/.secret"), "hidden\n");
    std::os::unix::fs::symlink(
        hub.knowledge.join("AGENTS.md"),
        hub.knowledge.join("skills/pdf/agents-link.md"),
    )
    .unwrap();
    let page = get(&hub, "/skills/private/pdf").await;
    assert_eq!(page.status, StatusCode::OK);
    assert!(
        page.body
            .contains("/skills/private/pdf/files/scripts/run.sh"),
        "{}",
        page.body
    );
    assert!(page.body.contains("It has scripts"));
    assert!(page.body.contains("data-copy="));
    let script = get(&hub, "/skills/private/pdf/files/scripts/run.sh").await;
    assert_eq!(script.status, StatusCode::OK);
    assert!(
        script.body.contains("echo &#60;b&#62;hi&#60;/b&#62;"),
        "{}",
        script.body
    );
    assert!(!script.body.contains("<b>hi"));
    assert!(script.body.contains("A script."));
    let md = get(&hub, "/skills/private/pdf/files/references/notes.md").await;
    assert_eq!(md.status, StatusCode::OK);
    assert!(md.body.contains("<h1>Notes</h1>"));
    assert!(!md.body.contains("<script>alert"));
    for path in [
        "/skills/private/pdf/files/.secret",
        "/skills/private/pdf/files/agents-link.md",
        "/skills/private/pdf/files/%2E%2E/%2E%2E/AGENTS.md",
        "/skills/private/pdf/files/..%2F..%2FAGENTS.md",
        "/skills/private/pdf/files/references/missing.md",
        "/skills/private/nope/files/SKILL.md",
    ] {
        let got = get(&hub, path).await;
        assert_eq!(got.status, StatusCode::NOT_FOUND, "{path}: {}", got.body);
    }
}

#[tokio::test]
async fn the_list_groups_marks_conflicts_and_filters_what_needs_attention() {
    let hub = hub();
    // Claude Code's own copy of keep, with different text.
    write(
        &hub.home.join(".claude/skills/keep/SKILL.md"),
        "---\ndescription: An older copy\n---\nOld\n",
    );
    let list = get(&hub, "/skills").await;
    assert_eq!(list.status, StatusCode::OK);
    assert!(list.body.contains("From acme/tools"), "{}", list.body);
    assert!(list.body.contains(">Conflict<"), "{}", list.body);
    let scoped = get(&hub, "/skills?scope=shop").await;
    assert!(scoped.body.contains("From acme/shop-tools"));
    let attention = get(&hub, "/skills?attention=1").await;
    assert!(
        attention
            .body
            .contains("Conflict with ~/.claude/skills/keep"),
        "{}",
        attention.body
    );
    assert!(!attention.body.contains(">tidy<"));
    let select = get(&hub, "/skills?select=1").await;
    assert!(
        select
            .body
            .contains("form=\"bulk\" name=\"s\" value=\"private/keep\"")
    );
    assert!(select.body.contains("id=\"bulk\""));
    let searched = get(&hub, "/skills?q=keep&attention=1").await;
    assert!(searched.body.contains("keep"));
    assert!(!searched.body.contains("value=\"private/pdf\""));
}

/// Links Knowledge's `name` into Claude Code's skills folder, as atlasd would, and makes
/// Codex's folder, so both agents are set up on the hub's machine.
fn claude_has(hub: &Hub, name: &str) {
    fs::create_dir_all(hub.home.join(".codex/skills")).unwrap();
    fs::create_dir_all(hub.home.join(".claude/skills")).unwrap();
    std::os::unix::fs::symlink(
        hub.knowledge.join("skills").join(name),
        hub.home.join(".claude/skills").join(name),
    )
    .unwrap();
}

#[tokio::test]
async fn nothing_is_flagged_missing_before_an_update_reaches_this_machine() {
    let hub = hub();
    claude_has(&hub, "pdf");
    let list = get(&hub, "/skills").await;
    assert_eq!(list.status, StatusCode::OK);
    // Codex lacks a, b, c and keep only because nothing has been sent yet.
    assert!(!list.body.contains("No agent has it"), "{}", list.body);
    assert!(!list.body.contains("Not in "), "{}", list.body);
    let attention = get(&hub, "/skills?attention=1").await;
    assert!(
        !attention.body.contains("value=\"private/a\""),
        "{}",
        attention.body
    );
    let home = get(&hub, "/").await;
    // This Knowledge has no remote, so Home says so and offers no Send that would fail.
    assert!(
        home.body
            .contains("get updates until Knowledge has a remote"),
        "{}",
        home.body
    );
    assert!(
        !home.body.contains(r#"data-action="send""#),
        "{}",
        home.body
    );
    assert!(!home.body.contains("Claude can"), "{}", home.body);
}

#[tokio::test]
async fn once_applied_only_what_the_owner_asked_for_and_is_missing_is_flagged() {
    let hub = hub_with(true);
    claude_has(&hub, "pdf");
    let list = get(&hub, "/skills").await;
    assert_eq!(list.status, StatusCode::OK);
    // a is wanted by every agent and none has it.
    assert!(list.body.contains("No agent has it"), "{}", list.body);
    // pdf is off for Codex, so Codex lacking it is what the owner asked for.
    assert!(!list.body.contains("Not in Codex"), "{}", list.body);
    let attention = get(&hub, "/skills?attention=1").await;
    assert!(attention.body.contains("private/a"), "{}", attention.body);
    assert!(
        !attention.body.contains("private/pdf"),
        "{}",
        attention.body
    );
    let home = get(&hub, "/").await;
    assert!(
        !home.body.contains("Nothing has been sent"),
        "{}",
        home.body
    );
}

#[tokio::test]
async fn bulk_and_new_skill_refusals_answer_the_page_script_in_json() {
    // A non-JSON answer makes passkey.js post the form again as a plain one.
    let hub = hub();
    let before = commits(&hub.knowledge);
    let is_json = |g: &Got| g.body.starts_with(r#"{"error":""#);
    let none = post_with(&hub, "/skills/bulk", &[("action", "on")], &hub.csrf, true).await;
    assert_eq!(none.status, StatusCode::BAD_REQUEST);
    assert!(is_json(&none), "{}", none.body);
    assert!(none.body.contains("Pick at least one skill first."));
    let stale = "0".repeat(64);
    let forbidden = post_with(
        &hub,
        "/skills/bulk",
        &[("action", "on"), ("s", "private/a")],
        &stale,
        true,
    )
    .await;
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    assert!(is_json(&forbidden), "{}", forbidden.body);
    let new = post_with(
        &hub,
        "/skills/new",
        &[
            ("name", "fresh"),
            ("description", "New"),
            ("where", "global"),
        ],
        &stale,
        true,
    )
    .await;
    assert_eq!(new.status, StatusCode::FORBIDDEN);
    assert!(is_json(&new), "{}", new.body);
    // A plain form post still gets text.
    let plain = post(&hub, "/skills/bulk", &[("action", "on")]).await;
    assert_eq!(plain.status, StatusCode::BAD_REQUEST);
    assert!(!plain.body.starts_with('{'));
    assert_eq!(commits(&hub.knowledge), before);
}
