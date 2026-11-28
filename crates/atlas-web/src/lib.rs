//! The hub's web UI.
//!
//! Every request reads a fresh snapshot of the store, so the pages always match git.
//! Access: the hub sits behind `tailscale serve`, which sets `Tailscale-User-Login`;
//! only logins in [`Access`] get in. Changes also need a passkey session ([`edit`]).

mod auth;
// Used by the skills, instructions and import pages as they land (decision 0027).
#[allow(dead_code)]
mod change;
pub mod clients;
mod distribute;
mod edit;
mod icons;
mod install;
mod library;
mod library_pages;
mod markdown;
pub mod release;
pub mod remote;
mod skill_ops;
#[allow(dead_code, clippy::unused_async)]
mod skills_place;
pub mod views;

use std::sync::Arc;

use askama::Template;
use atlas_fs::Sources;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use atlas_core::kernel::Scope;

pub use crate::edit::Writer;
use crate::views::{Page, Snapshot};

/// Who may use the hub: Tailscale logins such as `user@github`.
#[derive(Clone, Debug)]
pub struct Access {
    allowed: Vec<String>,
}

impl Access {
    #[must_use]
    pub fn new(allowed: Vec<String>) -> Self {
        Self { allowed }
    }

    fn allows(&self, headers: &HeaderMap) -> bool {
        headers
            .get("tailscale-user-login")
            .and_then(|v| v.to_str().ok())
            .is_some_and(|login| self.allowed.iter().any(|a| a == login))
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    sources: Arc<Sources>,
    hub: Arc<str>,
    /// Other machines, shown as not paired until phase 3.
    others: Arc<[String]>,
    /// Passkeys, sessions and saving. None keeps the hub read-only.
    writer: Option<Arc<Writer>>,
    /// The last read of the store, shared by requests for a few seconds.
    cache: Arc<tokio::sync::Mutex<Option<Cached>>>,
    /// The `skills` CLI's global lock on this hub (`atlas_fs::lock`).
    global_skill_lock: Arc<std::path::PathBuf>,
}

/// A read of the store and when it was made.
type Cached = (std::time::Instant, Arc<Snapshot>);

/// How long one read of the store serves requests. Changes made outside Atlas show up
/// within this; saves made through Atlas clear it at once.
const SNAPSHOT_TTL: std::time::Duration = std::time::Duration::from_secs(3);

impl AppState {
    #[must_use]
    pub fn new(sources: Sources, hub: &str, others: Vec<String>, writer: Option<Writer>) -> Self {
        let state = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from);
        let global_skill_lock = atlas_fs::lock::global_lock_path(&sources.home, state.as_deref());
        Self {
            global_skill_lock: Arc::new(global_skill_lock),
            sources: Arc::new(sources),
            hub: Arc::from(hub),
            others: others.into(),
            writer: writer.map(Arc::new),
            cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Reads the `skills` CLI's global lock from `path` instead of where the CLI keeps it
    /// for this login (`$XDG_STATE_HOME/skills` or `~/.agents`).
    #[must_use]
    pub fn with_global_skill_lock(mut self, path: std::path::PathBuf) -> Self {
        self.global_skill_lock = Arc::new(path);
        self
    }

    /// The store as of at most [`SNAPSHOT_TTL`] ago. Requests that arrive while it's being
    /// read wait for that one read instead of starting their own.
    async fn snapshot(&self) -> Result<Arc<Snapshot>, AppError> {
        let mut slot = self.cache.lock().await;
        if let Some((at, snap)) = slot.as_ref()
            && at.elapsed() < SNAPSHOT_TTL
        {
            return Ok(Arc::clone(snap));
        }
        let sources = Arc::clone(&self.sources);
        let lock = Arc::clone(&self.global_skill_lock);
        let snap = tokio::task::spawn_blocking(move || {
            let catalog = atlas_fs::load_catalog(&sources);
            let harnesses = atlas_fs::scan_harnesses(&sources);
            let library = library::Index::read(&sources, &lock, &catalog, &harnesses);
            Snapshot {
                catalog,
                harnesses,
                projects: atlas_fs::scan_projects(&sources),
                library,
            }
        })
        .await
        .map(Arc::new)
        .map_err(|_| AppError::Internal)?;
        *slot = Some((std::time::Instant::now(), Arc::clone(&snap)));
        Ok(snap)
    }

    /// Drops the cached read, so the next page shows a change just made.
    async fn invalidate(&self) {
        *self.cache.lock().await = None;
    }

    fn page(
        &self,
        headers: &HeaderMap,
        section: &'static str,
        title: impl Into<String>,
        snap: &Snapshot,
        scope: &Scope,
        scoped: bool,
    ) -> Page {
        let mut page = Page::new(section, title, &self.hub, &self.others, snap, scope, scoped);
        edit::decorate(&mut page, self, headers);
        page
    }
}

/// Builds the hub's router.
pub fn router(state: AppState, access: Access) -> Router {
    let access = Arc::new(access);
    Router::new()
        .route("/", get(overview))
        .route("/skills", get(skills))
        .route(
            "/skills/new",
            get(library_pages::new_form).post(library_pages::new_save),
        )
        .route("/skills/confirm", get(library_pages::confirm))
        .route("/skills/bulk", post(library_pages::bulk))
        .route(
            "/skills/{home}/{name}/files/{*path}",
            get(library_pages::file_view),
        )
        .route("/skills/{home}/{name}", get(skill_detail))
        .route("/instructions", get(instructions))
        .route("/instructions/{scope}/{file}", get(instruction_detail))
        .route("/memory", get(memory))
        .route("/memory/{name}", get(memory_detail))
        .route("/hooks", get(hooks))
        .route("/mcp", get(mcp))
        .route("/machines", get(machines))
        .route("/signin", get(edit::signin_page))
        .route("/setup/{token}", get(edit::setup_link))
        .route("/auth/enrol/start", post(edit::enrol_start))
        .route("/auth/enrol/finish", post(edit::enrol_finish))
        .route("/auth/signin/start", post(edit::signin_start))
        .route("/auth/signin/finish", post(edit::signin_finish))
        .route("/auth/signout", post(edit::signout))
        .route(
            "/settings",
            get(edit::settings_page).post(edit::settings_save),
        )
        .route("/settings/keys/{id}/remove", post(edit::remove_key))
        .route("/releases", get(distribute::releases_moved))
        .route("/send/start", post(distribute::send_start))
        .route("/send/finish", post(distribute::send_finish))
        .route("/machines/pair", post(distribute::pair_code))
        .route("/machines/pair/ssh", post(distribute::pair_ssh))
        .route("/machines/{id}/remove", post(distribute::unpair))
        .route("/install", get(install::script))
        .route("/install/{name}", get(install::binary))
        .route("/api/pair", post(distribute::api_pair))
        .route("/api/release", get(distribute::api_release))
        .route("/api/report", post(distribute::api_report))
        .route(
            "/skills/{home}/{name}/edit",
            get(edit::edit_skill).post(edit::save_skill),
        )
        .route(
            "/memory/{name}/edit",
            get(edit::edit_memory).post(edit::save_memory),
        )
        .route(
            "/instructions/{scope}/{file}/edit",
            get(edit::edit_instructions).post(edit::save_instructions),
        )
        .route("/static/{*path}", get(static_file))
        .fallback(not_found)
        .with_state(state)
        .layer(middleware::from_fn_with_state(access, require_login))
        .layer(middleware::from_fn(security_headers))
}

async fn require_login(State(access): State<Arc<Access>>, req: Request, next: Next) -> Response {
    if access.allows(req.headers()) {
        next.run(req).await
    } else {
        (
            StatusCode::FORBIDDEN,
            "Atlas is open only to its owner's Tailscale login.\n",
        )
            .into_response()
    }
}

const CSP: &str = "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; \
img-src 'self' data:; connect-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

async fn security_headers(req: Request, next: Next) -> Response {
    let is_static = req.uri().path().starts_with("/static/");
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CSP),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    h.insert(
        "permissions-policy",
        HeaderValue::from_static("camera=(), microphone=(), geolocation=(), interest-cohort=()"),
    );
    h.insert(
        "cross-origin-opener-policy",
        HeaderValue::from_static("same-origin"),
    );
    if !is_static {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    res
}

// ---------------------------------------------------------------- errors

#[derive(Debug)]
enum AppError {
    NotFound,
    Internal,
    /// A request Atlas refuses, with a reason the owner can act on.
    Bad(String),
}

impl From<askama::Error> for AppError {
    fn from(e: askama::Error) -> Self {
        tracing::error!("template failed: {e}");
        Self::Internal
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        match self {
            Self::NotFound => (StatusCode::NOT_FOUND, "Not in the atlas.\n").into_response(),
            Self::Bad(msg) => (StatusCode::BAD_REQUEST, format!("{msg}\n")).into_response(),
            Self::Internal => {
                (StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong.\n").into_response()
            }
        }
    }
}

type PageResult = Result<Html<String>, AppError>;

async fn not_found() -> AppError {
    AppError::NotFound
}

// ---------------------------------------------------------------- templates

#[derive(Template)]
#[template(path = "overview.html")]
struct OverviewPage {
    page: Page,
    v: views::Overview,
    /// "3 changes aren't on your machines yet", when some are waiting.
    waiting: Option<String>,
}

#[derive(Template)]
#[template(path = "skills.html")]
struct SkillsPage {
    page: Page,
    v: views::SkillsList,
}

#[derive(Template)]
#[template(path = "skill.html")]
struct SkillPage {
    page: Page,
    v: views::SkillDetail,
    files: library_pages::FileTree,
}

#[derive(Template)]
#[template(path = "instructions.html")]
struct InstructionsPage {
    page: Page,
    sections: Vec<views::InstructionSection>,
}

#[derive(Template)]
#[template(path = "instruction.html")]
struct InstructionPage {
    page: Page,
    v: views::InstructionDetail,
}

#[derive(Template)]
#[template(path = "memory.html")]
struct MemoryPage {
    page: Page,
    v: views::MemoryList,
}

#[derive(Template)]
#[template(path = "memory_page.html")]
struct MemoryDetailPage {
    page: Page,
    v: views::MemoryDetail,
}

#[derive(Template)]
#[template(path = "hooks.html")]
struct HooksPage {
    page: Page,
    v: views::Hooks,
}

#[derive(Template)]
#[template(path = "mcp.html")]
struct McpPage {
    page: Page,
}

#[derive(Template)]
#[template(path = "machines.html")]
struct MachinesPage {
    page: Page,
    machines: Vec<views::Machine>,
    pair_command: Option<String>,
    ssh_result: Option<SshResult>,
}

/// What an install over SSH from the Machines page came to.
pub(crate) struct SshResult {
    pub ok: bool,
    pub message: String,
    pub output: String,
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    scope: String,
    #[serde(default)]
    q: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    host: String,
    /// Skills: only those that need attention.
    #[serde(default)]
    attention: Option<String>,
    /// Skills: show checkboxes and the bulk bar.
    #[serde(default)]
    select: Option<String>,
    /// After a change through `change`: it was committed.
    #[serde(default)]
    changed: Option<String>,
    /// After a save: the commit, and what happened to the push.
    #[serde(default)]
    saved: String,
    #[serde(default)]
    pushed: Option<String>,
    #[serde(default)]
    pushfailed: Option<String>,
    #[serde(default)]
    unchanged: Option<String>,
    /// A public skill was saved: it reaches machines once pushed by hand.
    #[serde(default)]
    public: Option<String>,
    /// After sending: it went out, or the passkey prompt was closed.
    #[serde(default)]
    sent: Option<String>,
    #[serde(default)]
    notsent: Option<String>,
}

impl ListQuery {
    /// The flash line, and whether it offers a Send button.
    fn flash(&self) -> (String, bool) {
        if self.unchanged.is_some() {
            return (
                "Nothing changed, so nothing was committed.".to_owned(),
                false,
            );
        }
        let saved = !self.saved.is_empty() && self.saved.bytes().all(|b| b.is_ascii_hexdigit());
        if self.changed.is_some() && self.sent.is_none() {
            return (
                if self.notsent.is_some() {
                    "Done. Not on your machines yet".to_owned()
                } else {
                    "Done. Git keeps the old version if you need it back.".to_owned()
                },
                self.notsent.is_some(),
            );
        }
        if self.sent.is_some() {
            let text = if saved {
                "Saved · on your machines within 10 minutes"
            } else {
                "On your machines within 10 minutes"
            };
            return (text.to_owned(), false);
        }
        if !saved {
            return (String::new(), false);
        }
        if self.notsent.is_some() {
            return ("Saved. Not on your machines yet".to_owned(), true);
        }
        if self.public.is_some() {
            return (
                "Saved. Public skills reach your machines after you push them".to_owned(),
                false,
            );
        }
        let sha = &self.saved;
        let text = if self.pushed.is_some() {
            format!("Saved and pushed as {sha}.")
        } else if self.pushfailed.is_some() {
            format!("Saved as {sha}, but the push failed; it stays committed here.")
        } else {
            format!("Saved and committed as {sha}.")
        };
        (text, false)
    }

    fn apply(&self, page: &mut Page) {
        let (text, send) = self.flash();
        page.flash = text;
        page.flash_action = send.then_some("Send");
    }
}

async fn overview(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let mut page = s.page(&headers, "/", "Home", &snap, &Scope::Global, false);
    page.totals = format!("Hub · {}", page.hub);
    q.apply(&mut page);
    let waiting = distribute::waiting(&s).await.and_then(|w| w.text());
    Ok(Html(
        OverviewPage {
            page,
            v: views::overview(&snap),
            waiting,
        }
        .render()?,
    ))
}

async fn skills(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let scope = views::resolve_scope(&snap, &q.scope);
    let v = views::skills_library(
        &snap,
        &scope,
        &q.q,
        q.attention.is_some(),
        q.select.is_some(),
    );
    let mut page = s.page(&headers, "/skills", "Skills", &snap, &scope, true);
    page.totals = format!("{} skills", v.total);
    q.apply(&mut page);
    Ok(Html(SkillsPage { page, v }.render()?))
}

async fn skill_detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::skill_detail(&snap, &home, &name).ok_or(AppError::NotFound)?;
    let scope = views::resolve_scope(&snap, &home);
    let mut page = s.page(&headers, "/skills", v.name.clone(), &snap, &scope, false);
    q.apply(&mut page);
    page.edit_href = s
        .writer
        .is_some()
        .then(|| format!("/skills/{home}/{name}/edit"));
    let files = library_pages::file_tree(&snap, &home, &name, page.signed_in);
    Ok(Html(SkillPage { page, v, files }.render()?))
}

async fn instructions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let scope = views::resolve_scope(&snap, &q.scope);
    let mut page = s.page(
        &headers,
        "/instructions",
        "Instructions",
        &snap,
        &scope,
        true,
    );
    page.totals = format!("{} files", snap.catalog.instructions.len());
    Ok(Html(
        InstructionsPage {
            page,
            sections: views::instructions(&snap, &scope),
        }
        .render()?,
    ))
}

async fn instruction_detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((scope_key, file)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::instruction_detail(&snap, &scope_key, &file).ok_or(AppError::NotFound)?;
    let scope = views::resolve_scope(&snap, &scope_key);
    let mut page = s.page(
        &headers,
        "/instructions",
        v.title.clone(),
        &snap,
        &scope,
        false,
    );
    q.apply(&mut page);
    page.edit_href = s
        .writer
        .is_some()
        .then(|| format!("/instructions/{scope_key}/{file}/edit"));
    Ok(Html(InstructionPage { page, v }.render()?))
}

async fn memory(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let scope = views::resolve_scope(&snap, &q.scope);
    let v = views::memory(&snap, &scope, &q.q, &q.kind, &q.host);
    let mut page = s.page(&headers, "/memory", "Memory", &snap, &scope, true);
    page.totals = format!("{} notes", v.total);
    Ok(Html(MemoryPage { page, v }.render()?))
}

async fn memory_detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let scope = views::resolve_scope(&snap, &q.scope);
    let v = views::memory_detail(&snap, &scope, &name).ok_or(AppError::NotFound)?;
    let mut page = s.page(&headers, "/memory", v.name.clone(), &snap, &scope, false);
    q.apply(&mut page);
    // Agents' own per-repo notes aren't in git, so only store notes can be edited.
    page.edit_href =
        (s.writer.is_some() && scope == Scope::Global).then(|| format!("/memory/{name}/edit"));
    Ok(Html(MemoryDetailPage { page, v }.render()?))
}

async fn hooks(State(s): State<AppState>, headers: HeaderMap) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::hooks(&snap);
    let mut page = s.page(&headers, "/hooks", "Hooks", &snap, &Scope::Global, false);
    page.totals = format!("{} in Atlas · {} outside", v.atlas.len(), v.found.len());
    Ok(Html(HooksPage { page, v }.render()?))
}

async fn mcp(State(s): State<AppState>, headers: HeaderMap) -> PageResult {
    let snap = s.snapshot().await?;
    let page = s.page(
        &headers,
        "/mcp",
        "MCP servers",
        &snap,
        &Scope::Global,
        false,
    );
    Ok(Html(McpPage { page }.render()?))
}

async fn machines(State(s): State<AppState>, headers: HeaderMap) -> Result<Response, AppError> {
    machines_page(&s, &headers, None, None).await
}

/// The Machines page; `pair_command` shows a fresh pairing command once, `ssh_result`
/// how an install over SSH went.
pub(crate) async fn machines_page(
    s: &AppState,
    headers: &HeaderMap,
    pair_command: Option<String>,
    ssh_result: Option<SshResult>,
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let mut page = s.page(
        headers,
        "/machines",
        "Machines",
        &snap,
        &Scope::Global,
        false,
    );
    let (clients, latest) = s.writer.as_ref().map_or_else(
        || (Vec::new(), None),
        |w| (w.clients.list(), w.releases.numbers().last().copied()),
    );
    let machines = views::machines(&snap, &s.hub, &s.others, &clients, latest);
    let paired = machines
        .iter()
        .filter(|m| m.hub || !m.clients.is_empty())
        .count();
    page.totals = format!("{} machines · {paired} reporting", machines.len());
    Ok(Html(
        MachinesPage {
            page,
            machines,
            pair_command,
            ssh_result,
        }
        .render()?,
    )
    .into_response())
}

// ---------------------------------------------------------------- static files

const STATIC: &[(&str, &str, &[u8])] = &[
    (
        "atlas.css",
        "text/css; charset=utf-8",
        include_bytes!("../static/atlas.css"),
    ),
    (
        "htmx.min.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../static/htmx.min.js"),
    ),
    (
        "grain.svg",
        "image/svg+xml",
        include_bytes!("../static/grain.svg"),
    ),
    (
        "key.svg",
        "image/svg+xml",
        include_bytes!("../static/key.svg"),
    ),
    (
        "atlas.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../static/atlas.js"),
    ),
    (
        "passkey.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../static/passkey.js"),
    ),
    (
        "logo.png",
        "image/png",
        include_bytes!("../static/logo.png"),
    ),
    (
        "favicon.png",
        "image/png",
        include_bytes!("../static/favicon.png"),
    ),
    (
        "fonts/cinzel.woff2",
        "font/woff2",
        include_bytes!("../static/fonts/cinzel.woff2"),
    ),
    (
        "fonts/eb-garamond.woff2",
        "font/woff2",
        include_bytes!("../static/fonts/eb-garamond.woff2"),
    ),
    (
        "fonts/eb-garamond-italic.woff2",
        "font/woff2",
        include_bytes!("../static/fonts/eb-garamond-italic.woff2"),
    ),
    (
        "fonts/ibm-plex-mono-400.woff2",
        "font/woff2",
        include_bytes!("../static/fonts/ibm-plex-mono-400.woff2"),
    ),
    (
        "fonts/ibm-plex-mono-500.woff2",
        "font/woff2",
        include_bytes!("../static/fonts/ibm-plex-mono-500.woff2"),
    ),
];

async fn static_file(Path(path): Path<String>) -> Result<Response, AppError> {
    let (_, mime, bytes) = STATIC
        .iter()
        .find(|(p, ..)| *p == path)
        .ok_or(AppError::NotFound)?;
    // Fonts never change; everything else revalidates so a redeploy shows at once.
    let cache = if path.starts_with("fonts/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Ok((
        [
            (header::CONTENT_TYPE, *mime),
            (header::CACHE_CONTROL, cache),
        ],
        *bytes,
    )
        .into_response())
}
