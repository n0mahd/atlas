//! Check for updates against a stand-in for GitHub's API on 127.0.0.1: nothing here
//! reaches the network. Skills are installed from `acme/skills` at version 1; the
//! stand-in serves version 2.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use atlas_core::kernel::Slug;
use atlas_fs::lock::{TreeItem, computed_hash, git_blob_sha, git_tree_sha};
use atlas_fs::{Project, Sources};
use atlas_web::{Access, AppState, Writer, router};
use axum::body::{Body, to_bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use base64::Engine as _;
use tower::ServiceExt;

const ORIGIN: &str = "https://hub.test";
const LOGIN: &str = "owner@github";
const COOKIE: &str = "__Host-atlas";

// ---------------------------------------------------------------- the GitHub stand-in

type Reply = (u16, Vec<(String, String)>, String);

#[derive(Default)]
struct Canned {
    replies: HashMap<String, Reply>,
    hits: Mutex<BTreeMap<String, usize>>,
}

/// A folder's files: path, text, executable.
type Files<'a> = &'a [(&'a str, &'a str, bool)];

fn item(path: &str, text: &str, exec: bool) -> TreeItem {
    TreeItem {
        path: path.to_owned(),
        mode: if exec { "100755" } else { "100644" }.to_owned(),
        sha: git_blob_sha(text.as_bytes()),
    }
}

fn tree_sha(files: Files<'_>) -> String {
    let items: Vec<TreeItem> = files.iter().map(|(p, t, x)| item(p, t, *x)).collect();
    git_tree_sha(&items)
}

/// GitHub's recursive listing of these files: blobs plus a tree entry per folder.
fn listing(files: Files<'_>) -> (String, serde_json::Value) {
    let mut dirs: BTreeMap<String, Vec<(String, &str, bool)>> = BTreeMap::new();
    for (p, t, x) in files {
        let parts: Vec<&str> = p.split('/').collect();
        for i in 1..parts.len() {
            let dir = parts[..i].join("/");
            let rest = parts[i..].join("/");
            dirs.entry(dir).or_default().push((rest, t, *x));
        }
    }
    let mut entries = Vec::new();
    for (dir, inner) in &dirs {
        let items: Vec<TreeItem> = inner.iter().map(|(p, t, x)| item(p, t, *x)).collect();
        entries.push(serde_json::json!({
            "path": dir, "mode": "040000", "type": "tree", "sha": git_tree_sha(&items)
        }));
    }
    for (p, t, x) in files {
        let i = item(p, t, *x);
        entries.push(serde_json::json!({
            "path": p, "mode": i.mode, "type": "blob", "sha": i.sha, "size": t.len()
        }));
    }
    let sha = tree_sha(files);
    (
        sha.clone(),
        serde_json::json!({"sha": sha, "truncated": false, "tree": entries}),
    )
}

impl Canned {
    fn json(&mut self, path: &str, body: &serde_json::Value) {
        self.replies
            .insert(path.to_owned(), (200, Vec::new(), body.to_string()));
    }

    /// Serves `files` as `owner/repo`'s default branch, and every blob in it.
    fn repo(&mut self, repo: &str, files: Files<'_>) {
        let (_, body) = listing(files);
        self.json(&format!("/repos/{repo}/git/trees/HEAD?recursive=1"), &body);
        self.blobs(repo, files);
    }

    fn blobs(&mut self, repo: &str, files: Files<'_>) {
        for (_, t, _) in files {
            let content = base64::engine::general_purpose::STANDARD.encode(t);
            self.json(
                &format!("/repos/{repo}/git/blobs/{}", git_blob_sha(t.as_bytes())),
                &serde_json::json!({"content": content, "encoding": "base64"}),
            );
        }
    }

    /// Serves a folder's own listing by its tree SHA (the installed version).
    fn folder(&mut self, repo: &str, files: Files<'_>) {
        let (sha, body) = listing(files);
        self.json(&format!("/repos/{repo}/git/trees/{sha}?recursive=1"), &body);
        self.blobs(repo, files);
    }
}

async fn stand_in(canned: Canned) -> (String, Arc<Canned>) {
    let canned = Arc::new(canned);
    let shared = Arc::clone(&canned);
    let app = axum::Router::new().fallback(move |req: Request<Body>| {
        let canned = Arc::clone(&shared);
        async move {
            let key = req
                .uri()
                .path_and_query()
                .map_or_else(String::new, ToString::to_string);
            *canned.hits.lock().unwrap().entry(key.clone()).or_default() += 1;
            let (status, headers, body) = canned.replies.get(&key).cloned().unwrap_or((
                404,
                Vec::new(),
                "{\"message\":\"Not Found\"}".to_owned(),
            ));
            let mut res = axum::response::Response::builder().status(status);
            for (k, v) in headers {
                res = res.header(k, v);
            }
            res.body(Body::from(body)).unwrap()
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), canned)
}

// ---------------------------------------------------------------- the hub

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

struct Hub {
    _tmp: tempfile::TempDir,
    home: PathBuf,
    knowledge: PathBuf,
    project: PathBuf,
    app: axum::Router,
    cookie: String,
    csrf: String,
}

fn hub(api: &str, global_lock: impl Fn(&Path) -> PathBuf, setup: impl Fn(&Path, &Path)) -> Hub {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().to_path_buf();
    let knowledge = home.join("Knowledge");
    let project = home.join("Projects/shop");
    fs::create_dir_all(&knowledge).unwrap();
    fs::create_dir_all(&project).unwrap();
    setup(&knowledge, &project);
    repo(&knowledge, &[("AGENTS.md", "# Rules\n")]);
    repo(&project, &[("AGENTS.md", "# Shop\n")]);
    let sources = Sources {
        home: home.clone(),
        knowledge: knowledge.clone(),
        public_skills: None,
        projects: vec![Project {
            name: Slug::new("shop").unwrap(),
            root: project.clone(),
        }],
    };
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
        .with_github_stand_in(api, global_lock(&home));
    Hub {
        app: router(state, Access::new(vec![LOGIN.to_owned()])),
        _tmp: tmp,
        home,
        knowledge,
        project,
        cookie,
        csrf,
    }
}

impl Hub {
    async fn send(&self, req: Request<Body>) -> (StatusCode, String, String) {
        let res = self.app.clone().oneshot(req).await.unwrap();
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

    async fn get(&self, path: &str) -> (StatusCode, String) {
        let req = Request::get(path)
            .header("tailscale-user-login", LOGIN)
            .header(header::COOKIE, format!("{COOKIE}={}", self.cookie))
            .body(Body::empty())
            .unwrap();
        let (status, _, body) = self.send(req).await;
        (status, body)
    }

    async fn post(&self, path: &str, fields: &[(&str, &str)]) -> (StatusCode, String, String) {
        let mut all = vec![("csrf", self.csrf.as_str())];
        all.extend_from_slice(fields);
        let body = all
            .iter()
            .map(|(k, v)| format!("{k}={}", encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let req = Request::post(path)
            .header("tailscale-user-login", LOGIN)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::COOKIE, format!("{COOKIE}={}", self.cookie))
            .body(Body::from(body))
            .unwrap();
        self.send(req).await
    }

    async fn check(&self) -> String {
        let (status, location, _) = self.post("/skills/updates/check", &[]).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(location, "/skills/updates?checked=1");
        let (status, body) = self.get(&location).await;
        assert_eq!(status, StatusCode::OK);
        body
    }

    /// Opens the preview, then applies it with `choices`.
    async fn update(
        &self,
        home: &str,
        name: &str,
        choices: &[(&str, &str)],
    ) -> (StatusCode, String, String) {
        let path = format!("/skills/{home}/{name}/update");
        let (status, page) = self.get(&path).await;
        assert_eq!(status, StatusCode::OK);
        let local = hidden(&page, "local");
        let upstream = hidden(&page, "upstream");
        let mut fields = vec![("local", local.as_str()), ("upstream", upstream.as_str())];
        fields.extend_from_slice(choices);
        self.post(&path, &fields).await
    }
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn hidden(page: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let at = page
        .find(&marker)
        .unwrap_or_else(|| panic!("no {name} in {page}"))
        + marker.len();
    page[at..].split('"').next().unwrap().to_owned()
}

/// The text of one row of the Updates page.
fn row<'a>(page: &'a str, name: &str) -> &'a str {
    let at = page
        .find(&format!("<span class=\"row-name\">{name}</span>"))
        .unwrap_or_else(|| panic!("no row {name}"));
    let end = page[at..].find("</li>").unwrap();
    &page[at..at + end]
}

fn write_skill(dir: &Path, files: Files<'_>) {
    for (p, t, x) in files {
        write(&dir.join(p), t);
        if *x {
            use std::os::unix::fs::PermissionsExt as _;
            fs::set_permissions(dir.join(p), fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
}

// ---------------------------------------------------------------- versions

const BETA_V1: &str = "---\nname: beta\n---\none\ntwo\nthree\nfour\nfive\n";
const BETA_MINE: &str = "---\nname: beta\n---\nONE (mine)\ntwo\nthree\nfour\nfive\n";
const BETA_V2: &str = "---\nname: beta\n---\none\ntwo\nthree\nfour\nfive (theirs)\n";
const BETA_MERGED: &str = "---\nname: beta\n---\nONE (mine)\ntwo\nthree\nfour\nfive (theirs)\n";
const GAMMA_V1: &str = "---\nname: gamma\n---\nUse the old way.\n";
const GAMMA_MINE: &str = "---\nname: gamma\n---\nUse my way.\n";
const GAMMA_V2: &str = "---\nname: gamma\n---\nUse <script>alert(1)</script> the new way.\n";

fn global_lock(skills: &[(&str, &str)]) -> String {
    let entries: serde_json::Map<String, serde_json::Value> = skills
        .iter()
        .map(|(name, sha)| {
            (
                (*name).to_owned(),
                serde_json::json!({
                    "source": "acme/skills",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/acme/skills.git",
                    "skillPath": format!("skills/{name}/SKILL.md"),
                    "skillFolderHash": sha,
                    "installedAt": "2026-01-01T00:00:00.000Z",
                    "updatedAt": "2026-01-01T00:00:00.000Z"
                }),
            )
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "version": 3, "skills": entries, "dismissed": {"findSkillsPrompt": true}
    }))
    .unwrap()
}

/// Three global skills (unedited, edited apart, clashing) whose lock is in the knowledge
/// repo, so Atlas records the update there.
// One scenario from check to all-current, step by step.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn unedited_merged_and_clashing_skills_end_current() {
    let alpha_v1: Files<'_> = &[
        ("SKILL.md", "---\nname: alpha\n---\nA1\n", false),
        ("run.sh", "#!/bin/sh\necho 1\n", true),
    ];
    let alpha_v2: Files<'_> = &[
        ("SKILL.md", "---\nname: alpha\n---\nA2\n", false),
        ("run.sh", "#!/bin/sh\necho 2\n", true),
        ("notes.md", "New.\n", false),
    ];
    let beta_v1: Files<'_> = &[("SKILL.md", BETA_V1, false)];
    let gamma_v1: Files<'_> = &[("SKILL.md", GAMMA_V1, false)];

    let mut canned = Canned::default();
    let prefixed = |name: &str, files: Files<'_>| -> Vec<(String, String, bool)> {
        files
            .iter()
            .map(|(p, t, x)| (format!("skills/{name}/{p}"), (*t).to_owned(), *x))
            .collect()
    };
    let mut v2: Vec<(String, String, bool)> = prefixed("alpha", alpha_v2);
    v2.extend(prefixed("beta", &[("SKILL.md", BETA_V2, false)]));
    v2.extend(prefixed("gamma", &[("SKILL.md", GAMMA_V2, false)]));
    let v2_ref: Vec<(&str, &str, bool)> = v2
        .iter()
        .map(|(p, t, x)| (p.as_str(), t.as_str(), *x))
        .collect();
    canned.repo("acme/skills", &v2_ref);
    canned.folder("acme/skills", beta_v1);
    canned.folder("acme/skills", gamma_v1);
    let (api, served) = stand_in(canned).await;

    let lock_text = global_lock(&[
        ("alpha", &tree_sha(alpha_v1)),
        ("beta", &tree_sha(beta_v1)),
        ("gamma", &tree_sha(gamma_v1)),
    ]);
    let hub = hub(
        &api,
        |home| home.join("Knowledge/state/skills/.skill-lock.json"),
        |k, _| {
            write_skill(&k.join("skills/alpha"), alpha_v1);
            write_skill(&k.join("skills/beta"), &[("SKILL.md", BETA_MINE, false)]);
            write_skill(&k.join("skills/gamma"), &[("SKILL.md", GAMMA_MINE, false)]);
            write(&k.join("state/skills/.skill-lock.json"), &lock_text);
        },
    );

    // Reading the updates needs no request; the check lists the repo once.
    let page = hub.check().await;
    assert!(
        row(&page, "alpha").contains(">Update available · <span"),
        "{}",
        row(&page, "alpha")
    );
    assert!(row(&page, "beta").contains("Update available · you edited it"));
    assert!(row(&page, "gamma").contains("Update available · you edited it"));
    let hits = served.hits.lock().unwrap().clone();
    assert_eq!(
        hits.get("/repos/acme/skills/git/trees/HEAD?recursive=1"),
        Some(&1),
        "{hits:?}"
    );
    assert_eq!(hits.len(), 1, "a check is one request per repo: {hits:?}");

    // Preview: escaped diffs, nothing written yet.
    let (_, preview) = hub.get("/skills/private/alpha/update").await;
    assert!(
        preview.contains("Takes theirs") && preview.contains("New file"),
        "{preview}"
    );
    assert!(
        preview.contains("<span class=\"add\">+A2</span>"),
        "{preview}"
    );
    assert!(preview.contains("data-send-on-save"));
    assert!(
        fs::read_to_string(hub.knowledge.join("skills/alpha/SKILL.md"))
            .unwrap()
            .contains("A1")
    );
    let (_, preview) = hub.get("/skills/private/gamma/update").await;
    assert!(preview.contains("Clash: choose"), "{preview}");
    assert!(
        preview.contains("&#60;script&#62;alert(1)&#60;/script&#62;"),
        "{preview}"
    );
    assert!(
        !preview.contains("<script>alert"),
        "fetched text must be escaped"
    );

    // Unedited: theirs, execute bit kept, one commit with the lock.
    let (status, location, body) = hub.update("private", "alpha", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert!(
        location.starts_with("/skills/private/alpha/update?"),
        "{location}"
    );
    let alpha = hub.knowledge.join("skills/alpha");
    assert!(
        fs::read_to_string(alpha.join("SKILL.md"))
            .unwrap()
            .contains("A2")
    );
    assert_eq!(
        fs::read_to_string(alpha.join("notes.md")).unwrap(),
        "New.\n"
    );
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(alpha.join("run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "the script stays executable");
    }
    let log = git(&hub.knowledge, &["log", "-1", "--name-only", "--format=%s"]);
    assert!(log.starts_with("Update alpha from acme/skills"), "{log}");
    assert!(log.contains("state/skills/.skill-lock.json"), "{log}");
    let lock: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(hub.knowledge.join("state/skills/.skill-lock.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        lock["skills"]["alpha"]["skillFolderHash"],
        tree_sha(alpha_v2).as_str()
    );
    assert_eq!(
        lock["skills"]["beta"]["skillFolderHash"],
        tree_sha(beta_v1).as_str()
    );
    assert_eq!(lock["dismissed"]["findSkillsPrompt"], true);

    // A cancelled passkey tap offers Send again.
    let (_, done) = hub.get(&format!("{location}&notsent=1")).await;
    assert!(
        done.contains("Saved. Not on your machines yet") && done.contains("data-action=\"send\""),
        "{done}"
    );

    // Edited apart: merged, both edits kept.
    let (_, preview) = hub.get("/skills/private/beta/update").await;
    assert!(preview.contains("Merged with your edits"), "{preview}");
    let (status, _, body) = hub.update("private", "beta", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        fs::read_to_string(hub.knowledge.join("skills/beta/SKILL.md")).unwrap(),
        BETA_MERGED
    );

    // Clash: refused until the owner chooses, then theirs.
    let (status, _, body) = hub.update("private", "gamma", &[]).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert!(
        body.contains("Choose keep mine or take theirs for SKILL.md"),
        "{body}"
    );
    assert_eq!(
        fs::read_to_string(hub.knowledge.join("skills/gamma/SKILL.md")).unwrap(),
        GAMMA_MINE
    );
    let (status, _, body) = hub
        .update("private", "gamma", &[("p0", "SKILL.md"), ("c0", "theirs")])
        .await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        fs::read_to_string(hub.knowledge.join("skills/gamma/SKILL.md")).unwrap(),
        GAMMA_V2
    );
    assert_eq!(git(&hub.knowledge, &["status", "--porcelain"]), "");

    // All current afterwards, from the same listing.
    let (_, page) = hub.get("/skills/updates").await;
    assert!(
        row(&page, "alpha").contains(">Current · <span"),
        "{}",
        row(&page, "alpha")
    );
    assert!(row(&page, "beta").contains("Current, with your edits"));
    assert!(row(&page, "gamma").contains(">Current · <span"));
    assert!(!page.contains("Preview"));
    let trees = served
        .hits
        .lock()
        .unwrap()
        .get("/repos/acme/skills/git/trees/HEAD?recursive=1")
        .copied();
    assert_eq!(
        trees,
        Some(1),
        "previews and updates reuse the check's listing"
    );
}

/// A project skill (project lock, content hash) updates and its lock gets the hash the
/// CLI compares; a global lock outside every repo is left alone and the page says so.
// One scenario from check to all-current, step by step.
#[allow(clippy::too_many_lines)]
#[tokio::test]
async fn project_locks_are_written_and_outside_locks_left_alone() {
    let v1: Files<'_> = &[("SKILL.md", "---\nname: stock\n---\nCount.\n", false)];
    let v2: Files<'_> = &[
        ("SKILL.md", "---\nname: stock\n---\nCount twice.\n", false),
        ("metadata.json", "{}\n", false),
    ];
    let mut canned = Canned::default();
    let prefixed: Vec<(String, &str, bool)> = v2
        .iter()
        .map(|(p, t, x)| (format!("skills/stock/{p}"), *t, *x))
        .chain(std::iter::once((
            "skills/pdf/SKILL.md".to_owned(),
            "---\nname: pdf\n---\nPDF v2\n",
            false,
        )))
        .collect();
    let refs: Vec<(&str, &str, bool)> = prefixed
        .iter()
        .map(|(p, t, x)| (p.as_str(), *t, *x))
        .collect();
    canned.repo("acme/skills", &refs);
    let (api, _) = stand_in(canned).await;

    let hashed: Vec<(&str, &[u8])> = v1.iter().map(|(p, t, _)| (*p, t.as_bytes())).collect();
    let project_lock = format!(
        "{}\n",
        serde_json::to_string_pretty(&serde_json::json!({"version": 1, "skills": {
            "a-first": {"source": "acme/other", "sourceType": "github", "computedHash": "0".repeat(64)},
            "stock": {"source": "acme/skills", "sourceType": "github", "skillPath": "skills/stock/SKILL.md", "computedHash": computed_hash(&hashed)}
        }}))
        .unwrap()
    );
    let pdf_v1: Files<'_> = &[("SKILL.md", "---\nname: pdf\n---\nPDF v1\n", false)];
    let global = global_lock(&[("pdf", &tree_sha(pdf_v1))]);
    let hub = hub(
        &api,
        |home| home.join(".agents/.skill-lock.json"),
        |k, p| {
            write_skill(&p.join(".agents/skills/stock"), v1);
            write(&p.join("skills-lock.json"), &project_lock);
            write_skill(&k.join("skills/pdf"), pdf_v1);
        },
    );
    write(&hub.home.join(".agents/.skill-lock.json"), &global);

    let page = hub.check().await;
    assert!(
        row(&page, "stock").contains(">Update available · <span"),
        "{}",
        row(&page, "stock")
    );
    assert!(row(&page, "pdf").contains(">Update available · <span"));

    let (_, preview) = hub.get("/skills/shop/stock/update").await;
    assert!(
        preview.contains("skills-lock.json) is updated in the same change"),
        "{preview}"
    );
    assert!(
        !preview.contains("data-send-on-save"),
        "a project update sends nothing to machines"
    );
    let (status, location, body) = hub.update("shop", "stock", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    // The flash `change::respond` asks for, as on every other page.
    assert!(location.contains("saved="), "{location}");
    let (_, done) = hub.get(&location).await;
    assert!(done.contains("Saved and committed as "), "{done}");
    let lock_text = fs::read_to_string(hub.project.join("skills-lock.json")).unwrap();
    let v2_hashed: Vec<(&str, &[u8])> = v2.iter().map(|(p, t, _)| (*p, t.as_bytes())).collect();
    assert!(
        lock_text.contains(&computed_hash(&v2_hashed)),
        "{lock_text}"
    );
    assert!(lock_text.ends_with("}\n") && lock_text.find("a-first") < lock_text.find("\"stock\""));
    assert!(
        !hub.project
            .join(".agents/skills/stock/metadata.json")
            .exists(),
        "the CLI doesn't install it"
    );
    let log = git(&hub.project, &["log", "-1", "--name-only", "--format=%s"]);
    assert!(
        log.contains("skills-lock.json") && log.contains(".agents/skills/stock/SKILL.md"),
        "{log}"
    );

    let (_, preview) = hub.get("/skills/private/pdf/update").await;
    assert!(
        preview.contains("isn&#39;t in a repo Atlas commits"),
        "{preview}"
    );
    let (status, _, body) = hub.update("private", "pdf", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    assert_eq!(
        fs::read_to_string(hub.home.join(".agents/.skill-lock.json")).unwrap(),
        global
    );
    assert!(
        fs::read_to_string(hub.knowledge.join("skills/pdf/SKILL.md"))
            .unwrap()
            .contains("PDF v2")
    );

    let (_, page) = hub.get("/skills/updates").await;
    assert!(
        row(&page, "stock").contains(">Current · <span"),
        "{}",
        row(&page, "stock")
    );
    assert!(
        row(&page, "pdf").contains(">Current · <span"),
        "{}",
        row(&page, "pdf")
    );
}

/// GitHub's limit: a plain message with the reset time, and nothing more is asked until
/// then.
#[tokio::test]
async fn rate_limits_say_when_to_try_again() {
    let reset = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 600;
    let mut canned = Canned::default();
    canned.replies.insert(
        "/repos/acme/skills/git/trees/HEAD?recursive=1".to_owned(),
        (
            403,
            vec![
                ("x-ratelimit-remaining".to_owned(), "0".to_owned()),
                ("x-ratelimit-reset".to_owned(), reset.to_string()),
            ],
            "{\"message\":\"API rate limit exceeded\"}".to_owned(),
        ),
    );
    let (api, served) = stand_in(canned).await;
    let v1: Files<'_> = &[("SKILL.md", "---\nname: pdf\n---\nPDF\n", false)];
    let global = global_lock(&[("pdf", &tree_sha(v1))]);
    let hub = hub(
        &api,
        |home| home.join(".agents/.skill-lock.json"),
        |k, _| write_skill(&k.join("skills/pdf"), v1),
    );
    write(&hub.home.join(".agents/.skill-lock.json"), &global);

    let page = hub.check().await;
    let when = format!("{:02}:{:02}", (reset / 3600) % 24, (reset / 60) % 60);
    let message = format!("Try again after {when} UTC.");
    assert!(page.contains(&message), "{page}");
    assert!(row(&page, "pdf").contains("update-state warn"));
    hub.check().await;
    assert_eq!(
        served.hits.lock().unwrap().values().sum::<usize>(),
        1,
        "paused until the reset"
    );
}

#[tokio::test]
async fn updates_need_a_passkey_session() {
    let (api, _) = stand_in(Canned::default()).await;
    let hub = hub(
        &api,
        |home| home.join(".agents/.skill-lock.json"),
        |_, _| {},
    );
    let req = Request::get("/skills/updates")
        .header("tailscale-user-login", LOGIN)
        .body(Body::empty())
        .unwrap();
    let (status, location, _) = hub.send(req).await;
    assert_eq!(status, StatusCode::SEE_OTHER);
    assert!(location.starts_with("/signin"));
    let req = Request::post("/skills/updates/check")
        .header("tailscale-user-login", LOGIN)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, format!("{COOKIE}={}", hub.cookie))
        .body(Body::from("csrf=wrong"))
        .unwrap();
    assert_eq!(hub.send(req).await.0, StatusCode::FORBIDDEN);
}

/// A redirect is followed only to the API's own origin: a renamed repository on the same
/// stand-in is listed, one sent to another port (another origin) is never asked.
#[tokio::test]
async fn redirects_stay_on_the_apis_origin() {
    let v1: Files<'_> = &[("SKILL.md", "---\nname: pdf\n---\nPDF v1\n", false)];
    let v2: Files<'_> = &[(
        "skills/pdf/SKILL.md",
        "---\nname: pdf\n---\nPDF v2\n",
        false,
    )];
    let mut elsewhere = Canned::default();
    elsewhere.repo("acme/skills", v2);
    let (other, other_served) = stand_in(elsewhere).await;

    let mut canned = Canned::default();
    let moved = |to: String| (301, vec![("location".to_owned(), to)], String::new());
    canned.replies.insert(
        "/repos/acme/skills/git/trees/HEAD?recursive=1".to_owned(),
        moved(format!(
            "{other}/repos/acme/skills/git/trees/HEAD?recursive=1"
        )),
    );
    canned.replies.insert(
        "/repos/acme/renamed/git/trees/HEAD?recursive=1".to_owned(),
        moved("/repos/acme/skills-v2/git/trees/HEAD?recursive=1".to_owned()),
    );
    canned.repo("acme/skills-v2", v2);
    let (api, _) = stand_in(canned).await;

    let mut lock: serde_json::Value = serde_json::from_str(&global_lock(&[
        ("pdf", &tree_sha(v1)),
        ("docs", &tree_sha(v1)),
    ]))
    .unwrap();
    lock["skills"]["docs"]["source"] = "acme/renamed".into();
    lock["skills"]["docs"]["skillPath"] = "skills/pdf/SKILL.md".into();
    let hub = hub(
        &api,
        |home| home.join(".agents/.skill-lock.json"),
        |k, _| {
            write_skill(&k.join("skills/pdf"), v1);
            write_skill(&k.join("skills/docs"), v1);
        },
    );
    write(
        &hub.home.join(".agents/.skill-lock.json"),
        &lock.to_string(),
    );

    let page = hub.check().await;
    assert!(
        row(&page, "pdf").contains("GitHub couldn&#39;t be reached"),
        "{}",
        row(&page, "pdf")
    );
    assert!(
        other_served.hits.lock().unwrap().is_empty(),
        "another origin is never asked"
    );
    assert!(
        row(&page, "docs").contains(">Update available · <span"),
        "{}",
        row(&page, "docs")
    );
}

/// A script that only became executable upstream is an update, and updating sets the bit.
#[tokio::test]
async fn an_execute_bit_change_is_an_update() {
    let v1: Files<'_> = &[
        ("SKILL.md", "---\nname: tool\n---\nRun it.\n", false),
        ("run.sh", "#!/bin/sh\necho hi\n", false),
    ];
    let v2: Files<'_> = &[
        (
            "skills/tool/SKILL.md",
            "---\nname: tool\n---\nRun it.\n",
            false,
        ),
        ("skills/tool/run.sh", "#!/bin/sh\necho hi\n", true),
    ];
    let mut canned = Canned::default();
    canned.repo("acme/skills", v2);
    let (api, _) = stand_in(canned).await;
    let global = global_lock(&[("tool", &tree_sha(v1))]);
    let hub = hub(
        &api,
        |home| home.join(".agents/.skill-lock.json"),
        |k, _| write_skill(&k.join("skills/tool"), v1),
    );
    write(&hub.home.join(".agents/.skill-lock.json"), &global);

    let page = hub.check().await;
    assert!(
        row(&page, "tool").contains(">Update available · <span"),
        "{}",
        row(&page, "tool")
    );
    let (_, preview) = hub.get("/skills/private/tool/update").await;
    assert!(
        preview.contains("Only its execute bit changes."),
        "{preview}"
    );
    let (status, _, body) = hub.update("private", "tool", &[]).await;
    assert_eq!(status, StatusCode::SEE_OTHER, "{body}");
    use std::os::unix::fs::PermissionsExt as _;
    let mode = fs::metadata(hub.knowledge.join("skills/tool/run.sh"))
        .unwrap()
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "the script is executable now");
    let (_, page) = hub.get("/skills/updates").await;
    assert!(
        row(&page, "tool").contains(">Current · <span"),
        "{}",
        row(&page, "tool")
    );
}

/// A skill named `edit` links to its own update page.
#[tokio::test]
async fn a_skill_named_edit_links_to_its_update_page() {
    let (api, _) = stand_in(Canned::default()).await;
    let hub = hub(
        &api,
        |home| home.join(".agents/.skill-lock.json"),
        |k, _| {
            write_skill(
                &k.join("skills/edit"),
                &[("SKILL.md", "---\nname: edit\n---\nEdit.\n", false)],
            )
        },
    );
    let (status, page) = hub.get("/skills/private/edit").await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert!(
        page.contains("href=\"/skills/private/edit/update\">Check for updates"),
        "{page}"
    );
}
