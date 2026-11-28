//! The Skills page's per-agent switches and "Use in…", end to end through the router:
//! real git repos in a temp folder, a signed-in session, and the files and commits on disk.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use atlas_core::kernel::Slug;
use atlas_fs::{Project, Sources};
use atlas_web::{Access, AppState, Writer, router};
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode, header};
use tower::ServiceExt;

const ORIGIN: &str = "https://hub.test";
const LOGIN: &str = "owner@github";
const COOKIE: &str = "__Host-atlas";
const PLACEMENTS: &str = "atlas/placements.toml";

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
    knowledge: PathBuf,
    app: axum::Router,
    cookie: String,
    csrf: String,
}

/// A hub whose machine has `agents` set up (by their marker folders), a knowledge repo
/// with `pdf-tools` and `stock`, and two projects: `shop` (with its own `stock`) and
/// `cafe`.
async fn hub(agents: &[&str]) -> Hub {
    hub_with(agents, 0).await
}

/// Like [`hub`], with `extra` more global skills.
async fn hub_with(agents: &[&str], extra: usize) -> Hub {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    for marker in agents {
        fs::create_dir_all(home.join(marker)).unwrap();
    }
    let knowledge = home.join("Knowledge");
    repo(
        &knowledge,
        &[
            ("AGENTS.md", "# Rules\n"),
            (
                "skills/pdf-tools/SKILL.md",
                "---\ndescription: Read PDFs\n---\nBody\n",
            ),
            (
                "skills/stock/SKILL.md",
                "---\ndescription: The global stock\n---\nBody\n",
            ),
            ("memory/other.md", "---\nname: other\n---\nUntouched.\n"),
        ],
    );
    for i in 0..extra {
        let dir = knowledge.join(format!("skills/skill-{i}"));
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            "---\ndescription: One of many\n---\nBody\n",
        )
        .unwrap();
    }
    let shop = home.join("Projects/shop");
    repo(
        &shop,
        &[
            ("AGENTS.md", "# Shop\n"),
            (
                ".agents/skills/stock/SKILL.md",
                "---\ndescription: Count\n---\nBody\n",
            ),
        ],
    );
    let cafe = home.join("Projects/cafe");
    repo(&cafe, &[("AGENTS.md", "# Cafe\n")]);
    let sources = Sources {
        home: home.clone(),
        knowledge: knowledge.clone(),
        public_skills: None,
        projects: vec![
            Project {
                name: Slug::new("cafe").unwrap(),
                root: cafe,
            },
            Project {
                name: Slug::new("shop").unwrap(),
                root: shop,
            },
        ],
    };
    let writer = Writer::open(ORIGIN, &home.join("state"), Duration::from_mins(30)).unwrap();
    let cookie = writer.auth.sign_in();
    let app = router(
        AppState::new(sources, "test-hub", Vec::new(), Some(writer)),
        Access::new(vec![LOGIN.to_owned()]),
    );
    let mut hub = Hub {
        _tmp: tmp,
        knowledge,
        app,
        cookie,
        csrf: String::new(),
    };
    let (_, _, page) = send(&hub, get(&hub, "/skills", true)).await;
    let at = page.find(r#"name="atlas-csrf" content=""#).unwrap() + 27;
    page[at..at + 64].clone_into(&mut hub.csrf);
    hub
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

fn get(hub: &Hub, path: &str, signed_in: bool) -> Request<Body> {
    let mut req = Request::get(path).header("tailscale-user-login", LOGIN);
    if signed_in {
        req = req.header(header::COOKIE, format!("{COOKIE}={}", hub.cookie));
    }
    req.body(Body::empty()).unwrap()
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

fn post(hub: &Hub, path: &str, fields: &[(&str, &str)]) -> Request<Body> {
    let body: String = fields
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&");
    Request::post(path)
        .header("tailscale-user-login", LOGIN)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("{COOKIE}={}", hub.cookie))
        .body(Body::from(body))
        .unwrap()
}

fn head(hub: &Hub) -> String {
    git(&hub.knowledge, &["rev-parse", "HEAD"])
}

fn placements(hub: &Hub) -> Option<String> {
    fs::read_to_string(hub.knowledge.join(PLACEMENTS)).ok()
}

/// The `<button>` of the switch whose label starts with `label`.
fn switch<'a>(page: &'a str, label: &str) -> &'a str {
    let at = page
        .find(&format!(r#"aria-label="{label}"#))
        .unwrap_or_else(|| panic!("no switch {label}"));
    let start = page[..at].rfind("<button").unwrap();
    let end = at + page[at..].find('>').unwrap();
    &page[start..=end]
}

#[tokio::test]
async fn a_switch_writes_the_placements_file_and_commits_exactly_it() {
    let hub = hub(&[".claude", ".codex"]).await;
    fs::write(hub.knowledge.join("memory/other.md"), "dirty, not ours\n").unwrap();
    let (_, _, page) = send(&hub, get(&hub, "/skills/private/pdf-tools", true)).await;
    assert!(switch(&page, "Codex uses pdf-tools").contains(r#"aria-checked="true""#));
    assert!(
        page.contains("data-send-on-save"),
        "sending is on by default"
    );

    let (status, location, _) = send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/agents",
            &[
                ("csrf", &hub.csrf),
                ("agent", "codex"),
                ("on", "0"),
                ("back", "page"),
                ("scope", "global"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(
        location
            .starts_with("/skills/private/pdf-tools?scope=global&turned=off&agent=codex&saved="),
        "{location}"
    );
    let text = placements(&hub).unwrap();
    assert!(
        text.contains("[global.pdf-tools]\noff = [\"codex\"]"),
        "{text}"
    );
    assert_eq!(
        git(&hub.knowledge, &["log", "-1", "--format=%s"]).trim(),
        "Turn pdf-tools off for Codex in Atlas"
    );
    assert_eq!(
        git(
            &hub.knowledge,
            &["show", "--name-only", "--format=", "HEAD"]
        )
        .trim(),
        PLACEMENTS
    );
    assert_eq!(
        fs::read_to_string(hub.knowledge.join("memory/other.md")).unwrap(),
        "dirty, not ours\n",
        "an unrelated edit stays out of the commit"
    );

    let (_, _, page) = send(&hub, get(&hub, &location, true)).await;
    assert!(page.contains("Turned off for Codex"), "flash");
    assert!(switch(&page, "Codex uses pdf-tools").contains(r#"aria-checked="false""#));
    assert!(switch(&page, "Claude uses pdf-tools").contains(r#"aria-checked="true""#));
    assert!(page.contains("On for 1 of 2 agents"));

    // The same switch again changes nothing and commits nothing.
    let before = head(&hub);
    let (_, location, _) = send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/agents",
            &[
                ("csrf", &hub.csrf),
                ("agent", "codex"),
                ("on", "0"),
                ("back", "page"),
            ],
        ),
    )
    .await;
    assert!(location.ends_with("&unchanged=1"), "{location}");
    assert_eq!(head(&hub), before);
    let (_, _, page) = send(&hub, get(&hub, &location, true)).await;
    assert!(page.contains("Nothing changed"));

    // Back on: the entry goes, so the file lists only exceptions.
    send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/agents",
            &[("csrf", &hub.csrf), ("agent", "codex"), ("on", "1")],
        ),
    )
    .await;
    assert!(!placements(&hub).unwrap().contains("pdf-tools"));
}

#[tokio::test]
async fn the_page_script_gets_json_and_plain_forms_get_a_redirect() {
    let hub = hub(&[".claude", ".codex"]).await;
    let mut req = post(
        &hub,
        "/skills/private/pdf-tools/agents",
        &[
            ("csrf", &hub.csrf),
            ("agent", "claude-code"),
            ("on", "0"),
            ("back", "list"),
            ("scope", "shop"),
        ],
    );
    req.headers_mut()
        .insert(header::ACCEPT, "application/json".parse().unwrap());
    let (status, _, body) = send(&hub, req).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.starts_with(
            r#"{"next":"/skills?scope=shop&open=private/pdf-tools&turned=off&agent=claude-code&saved="#
        ) && body.ends_with(r#"","send":true}"#),
        "{body}"
    );
    // Without script: a redirect to the list with the row open and the flash.
    let (status, location, _) = send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/agents",
            &[
                ("csrf", &hub.csrf),
                ("agent", "claude-code"),
                ("on", "1"),
                ("back", "list"),
                ("scope", "shop"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let (_, _, list) = send(&hub, get(&hub, &location, true)).await;
    assert!(list.contains("Turned on for Claude"));
    assert!(
        list.contains(r#"<details class="row" open>"#),
        "row reopened"
    );
    assert!(list.contains("autofocus"), "focus returns to the row");
}

#[tokio::test]
async fn a_stale_token_or_another_origin_writes_nothing() {
    let hub = hub(&[".claude", ".codex"]).await;
    let before = head(&hub);
    let fields = [("agent", "codex"), ("on", "0")];
    let mut stale = vec![("csrf", "0".repeat(64))];
    stale.extend(fields.iter().map(|(k, v)| (*k, (*v).to_owned())));
    let stale: Vec<(&str, &str)> = stale.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let (status, ..) = send(&hub, post(&hub, "/skills/private/pdf-tools/agents", &stale)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);

    let mut other = post(
        &hub,
        "/skills/private/pdf-tools/agents",
        &[("csrf", &hub.csrf), ("agent", "codex"), ("on", "0")],
    );
    other
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.test".parse().unwrap());
    assert_eq!(send(&hub, other).await.0, StatusCode::FORBIDDEN);

    let mut signed_out = post(
        &hub,
        "/skills/private/pdf-tools/use",
        &[("csrf", &hub.csrf), ("use", "only"), ("projects", "shop")],
    );
    signed_out.headers_mut().remove(header::COOKIE);
    assert_eq!(send(&hub, signed_out).await.0, StatusCode::FORBIDDEN);

    assert!(placements(&hub).is_none());
    assert_eq!(head(&hub), before);
}

#[tokio::test]
async fn unknown_agents_skills_and_projects_are_refused() {
    let hub = hub(&[".claude", ".codex"]).await;
    let before = head(&hub);
    for agent in ["t3", "herdr", "nobody", "", "Codex"] {
        let (status, ..) = send(
            &hub,
            post(
                &hub,
                "/skills/private/pdf-tools/agents",
                &[("csrf", &hub.csrf), ("agent", agent), ("on", "0")],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{agent}");
    }
    let (status, ..) = send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/agents",
            &[("csrf", &hub.csrf), ("agent", "codex"), ("on", "yes")],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "on must be 0 or 1");
    for path in [
        "/skills/private/nope/agents",
        "/skills/nowhere/pdf-tools/agents",
        "/skills/private/..%2Fpdf-tools/agents",
    ] {
        let (status, ..) = send(
            &hub,
            post(
                &hub,
                path,
                &[("csrf", &hub.csrf), ("agent", "codex"), ("on", "0")],
            ),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{path}");
    }
    for fields in [
        vec![("use", "only"), ("projects", "elsewhere")],
        vec![("use", "only")],
        vec![("use", "project"), ("project", "../shop")],
        vec![("use", "sideways")],
    ] {
        let mut all = vec![("csrf", hub.csrf.as_str())];
        all.extend(fields.iter().copied());
        let (status, ..) = send(&hub, post(&hub, "/skills/private/pdf-tools/use", &all)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{fields:?}");
    }
    assert!(placements(&hub).is_none());
    assert_eq!(head(&hub), before);
}

#[tokio::test]
async fn an_agent_that_cannot_be_kept_away_shows_why() {
    // In a project, the skill sits in .agents/skills, which Codex and Grok Build both
    // read; Grok has no setting to turn a skill off.
    let hub = hub(&[".codex", ".grok"]).await;
    let (_, _, page) = send(&hub, get(&hub, "/skills/shop/stock", true)).await;
    let grok = switch(&page, "Grok uses stock");
    assert!(grok.contains(" disabled"), "{grok}");
    assert!(grok.contains(r#"aria-describedby="why-shop--stock-grok""#));
    assert!(page.contains(
        "Can&#39;t be turned off here: Grok reads the same folder as Codex and has no setting for this."
    ) || page.contains(
        "Can&#x27;t be turned off here: Grok reads the same folder as Codex and has no setting for this."
    ), "reason missing");
    // Codex switches a project's skill off from its home config.toml (keyed by path), so
    // in a repo it can still be turned off.
    assert!(!switch(&page, "Codex uses stock").contains(" disabled"));
    // For a global skill Codex has its own folder and Grok has .grok/skills: both free.
    let (_, _, page) = send(&hub, get(&hub, "/skills/private/pdf-tools", true)).await;
    assert!(!switch(&page, "Codex uses pdf-tools").contains(" disabled"));
    assert!(!switch(&page, "Grok uses pdf-tools").contains(" disabled"));
}

#[tokio::test]
async fn only_these_projects_round_trips() {
    let hub = hub(&[".claude", ".codex"]).await;
    let (status, location, _) = send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/use",
            &[
                ("csrf", &hub.csrf),
                ("use", "only"),
                ("projects", "shop"),
                ("projects", "cafe"),
                ("back", "list"),
                ("scope", "global"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let text = placements(&hub).unwrap();
    assert!(
        text.contains("[global.pdf-tools]\nonly-projects = [\n    \"cafe\",\n    \"shop\",\n]")
            || text.contains("only-projects = [\"cafe\", \"shop\"]"),
        "{text}"
    );
    assert_eq!(
        git(&hub.knowledge, &["log", "-1", "--format=%s"]).trim(),
        "Use pdf-tools only in cafe and shop in Atlas"
    );
    let (_, _, list) = send(&hub, get(&hub, &location, true)).await;
    assert!(list.contains("Now used only in the projects you chose"));
    assert!(list.contains(r#"<span class="row-meta place-badge">2 projects</span>"#));
    let (_, _, page) = send(&hub, get(&hub, "/skills/private/pdf-tools", true)).await;
    assert!(page.contains(r#"name="use" value="only" checked"#));
    assert!(page.contains(r#"name="projects" value="shop" checked"#));

    let (_, location, _) = send(
        &hub,
        post(
            &hub,
            "/skills/private/pdf-tools/use",
            &[("csrf", &hub.csrf), ("use", "global"), ("back", "page")],
        ),
    )
    .await;
    assert!(location.contains("used=global"), "{location}");
    assert!(!placements(&hub).unwrap().contains("pdf-tools"));
    let (_, _, list) = send(&hub, get(&hub, "/skills", true)).await;
    assert!(!list.contains("place-badge"));
}

#[tokio::test]
async fn a_refused_move_says_why_and_changes_nothing() {
    let hub = hub(&[".claude", ".codex"]).await;
    let before = head(&hub);
    // `shop` already has a skill named stock, so moving the global one there is refused
    // (and moving isn't built yet on this branch; either way nothing may change).
    let (status, _, page) = send(
        &hub,
        post(
            &hub,
            "/skills/private/stock/use",
            &[
                ("csrf", &hub.csrf),
                ("use", "project"),
                ("project", "shop"),
                ("back", "page"),
            ],
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(page.contains(r#"<p class="marginalia warn-note" role="alert">"#));
    assert!(page.contains("Nothing changed") || page.contains("nothing changed"));
    assert!(page.contains(r#"href="/skills/private/stock?scope=global""#));
    assert_eq!(head(&hub), before);
    assert!(hub.knowledge.join("skills/stock/SKILL.md").exists());
}

#[tokio::test]
async fn turning_a_skill_off_for_every_agent_asks_first() {
    let hub = hub(&[".claude", ".codex"]).await;
    let ask = [
        ("csrf", hub.csrf.as_str()),
        ("agent", "all"),
        ("on", "0"),
        ("back", "page"),
    ];
    let (status, _, page) = send(&hub, post(&hub, "/skills/private/pdf-tools/agents", &ask)).await;
    assert_eq!(status, StatusCode::OK);
    assert!(page.contains("This turns pdf-tools off for Claude and Codex."));
    assert!(page.contains(r#"name="confirmed" value="1""#));
    assert!(placements(&hub).is_none(), "asking writes nothing");

    let mut confirmed = ask.to_vec();
    confirmed.push(("confirmed", "1"));
    let (status, location, _) = send(
        &hub,
        post(&hub, "/skills/private/pdf-tools/agents", &confirmed),
    )
    .await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    let text = placements(&hub).unwrap();
    for id in ["claude-code", "codex", "grok", "pi"] {
        assert!(text.contains(&format!("\"{id}\"")), "{id} not off: {text}");
    }
    let (_, _, page) = send(&hub, get(&hub, &location, true)).await;
    assert!(page.contains("Turned off for every agent"));
    assert!(page.contains("Off for every agent here"));
    assert!(switch(&page, "Every agent uses pdf-tools").contains(r#"aria-checked="false""#));
}

#[tokio::test]
async fn signed_out_the_switches_show_but_cannot_change() {
    let hub = hub(&[".claude", ".codex"]).await;
    let (_, _, page) = send(&hub, get(&hub, "/skills/private/pdf-tools", false)).await;
    assert!(switch(&page, "Codex uses pdf-tools").contains(" disabled"));
    assert!(page.contains(r#"<a href="/signin?next=/skills/private/pdf-tools">Sign in</a>"#));
}

#[tokio::test]
async fn a_large_store_renders_quickly() {
    let hub = hub_with(
        &[
            ".claude",
            ".codex",
            ".config/opencode",
            ".pi/agent",
            ".gemini",
            ".config/amp",
            ".config/muse",
            ".grok",
        ],
        120,
    )
    .await;
    // Setting up read the store once; time the next request, which is the page work.
    let started = std::time::Instant::now();
    let (status, _, page) = send(&hub, get(&hub, "/skills", true)).await;
    let took = started.elapsed();
    assert_eq!(status, StatusCode::OK);
    assert!(page.matches(r#"role="switch""#).count() >= 120 * 9);
    // Generous for an unoptimised test build; a release build renders this far faster.
    assert!(took < Duration::from_millis(500), "{took:?}");
}
