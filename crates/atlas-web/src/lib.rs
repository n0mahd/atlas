//! The hub's web UI.
//!
//! Every request reads a fresh snapshot of the store, so the pages always match git.
//! Access: the hub sits behind `tailscale serve`, which sets `Tailscale-User-Login`;
//! only logins in [`Access`] get in. Changes also need a passkey session ([`edit`]).

mod auth;
mod change;
pub mod clients;
mod distribute;
mod edit;
mod icons;
mod import;
mod install;
mod instructions_manage;
mod library;
mod library_pages;
mod markdown;
pub mod release;
pub mod remote;
mod review;
mod skill_ops;
mod skills_place;
#[cfg(test)]
mod test_passkey;
mod tidy;
pub mod tokens;
mod updates;
pub mod views;

use std::sync::Arc;

use askama::Template;
use atlas_fs::Sources;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
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
    /// Check for updates: the GitHub reader and the last check (`updates`).
    updates: Arc<updates::Updates>,
}

/// A read of the store and when it was made.
#[derive(Debug)]
struct Cached {
    at: std::time::Instant,
    snap: Arc<Snapshot>,
    /// The Instructions pages' facts about this machine, read on first use with this
    /// snapshot and dropped with it.
    instructions: instructions_manage::FactsSlot,
}

/// How long one read of the store serves requests. Changes made outside Atlas show up
/// within this; saves made through Atlas clear it at once.
const SNAPSHOT_TTL: std::time::Duration = std::time::Duration::from_secs(3);

impl AppState {
    #[must_use]
    pub fn new(sources: Sources, hub: &str, others: Vec<String>, writer: Option<Writer>) -> Self {
        let updates = Arc::new(updates::Updates::new(&sources));
        if let Some(w) = &writer {
            w.machines.seed(&others);
        }
        Self {
            updates,
            sources: Arc::new(sources),
            hub: Arc::from(hub),
            others: others.into(),
            writer: writer.map(Arc::new),
            cache: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Where the `skills` CLI keeps its global lock on this machine. The binary works it
    /// out (the CLI honours `XDG_STATE_HOME`); without this, `~/.agents/.skill-lock.json`.
    #[must_use]
    pub fn with_skills_lock(mut self, global_lock: std::path::PathBuf) -> Self {
        self.updates = Arc::new(updates::Updates::with(updates::GITHUB_API, global_lock));
        self
    }

    /// For tests only: reads skill sources from a stand-in for GitHub's API on
    /// `http://127.0.0.1:<port>` and the `skills` CLI's global lock from `global_lock`,
    /// so a test never reaches the network or the machine's own lock.
    ///
    /// # Panics
    /// If `api` isn't a loopback `http://127.0.0.1:` address.
    #[cfg(any(test, feature = "testing"))]
    #[doc(hidden)]
    #[must_use]
    pub fn with_github_stand_in(mut self, api: &str, global_lock: std::path::PathBuf) -> Self {
        assert!(
            api.starts_with("http://127.0.0.1:"),
            "the GitHub stand-in must be on 127.0.0.1"
        );
        self.updates = Arc::new(updates::Updates::with(api, global_lock));
        self
    }

    pub(crate) fn known_machines(&self) -> Vec<String> {
        self.writer
            .as_ref()
            .map_or_else(|| self.others.to_vec(), |w| w.machines.list())
    }

    /// The store as of at most [`SNAPSHOT_TTL`] ago. Requests that arrive while it's being
    /// read wait for that one read instead of starting their own.
    async fn snapshot(&self) -> Result<Arc<Snapshot>, AppError> {
        Ok(self.cached().await?.0)
    }

    /// [`AppState::snapshot`] with the slot that keeps the Instructions pages' facts for
    /// as long as that snapshot serves.
    async fn cached(&self) -> Result<(Arc<Snapshot>, instructions_manage::FactsSlot), AppError> {
        let mut slot = self.cache.lock().await;
        if let Some(c) = slot.as_ref()
            && c.at.elapsed() < SNAPSHOT_TTL
        {
            return Ok((Arc::clone(&c.snap), Arc::clone(&c.instructions)));
        }
        let sources = Arc::clone(&self.sources);
        let lock = self.updates.global_lock().to_path_buf();
        let snap = tokio::task::spawn_blocking(move || {
            let catalog = atlas_fs::load_catalog(&sources);
            let harnesses = atlas_fs::scan_harnesses(&sources);
            let library = library::Index::read(&sources, &lock, &catalog, &harnesses);
            let outside = atlas_fs::outside::scan(
                &sources,
                &catalog,
                &harnesses,
                &atlas_fs::outside::Rules::default(),
            );
            Snapshot {
                catalog,
                harnesses,
                projects: atlas_fs::scan_projects(&sources),
                library,
                outside,
                delivered: false,
            }
        })
        .await
        .map_err(|_| AppError::Internal)?;
        let snap = Arc::new(Snapshot {
            delivered: distribute::delivered(self).await,
            ..snap
        });
        let instructions = instructions_manage::FactsSlot::default();
        *slot = Some(Cached {
            at: std::time::Instant::now(),
            snap: Arc::clone(&snap),
            instructions: Arc::clone(&instructions),
        });
        Ok((snap, instructions))
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
        let others = self.known_machines();
        let clients = self
            .writer
            .as_ref()
            .map_or_else(Vec::new, |w| w.clients.list());
        let mut page = Page::new(
            section, title, &self.hub, &others, &clients, snap, scope, scoped,
        );
        edit::decorate(&mut page, self, headers);
        page
    }
}

/// The skills, instructions and import pages (decision 0027).
fn catalog_routes() -> Router<AppState> {
    Router::new()
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
        .route("/skills/tidy", get(tidy::page).post(tidy::run))
        .route("/skills/{home}/{name}", get(skill_detail))
        .route(
            "/skills/{home}/{name}/agents",
            post(skills_place::post_agents),
        )
        .route("/skills/{home}/{name}/use", post(skills_place::post_use))
        .route("/skills/updates", get(updates::list))
        .route("/skills/updates/check", post(updates::check))
        .route(
            "/skills/{home}/{name}/update",
            get(updates::preview).post(updates::apply),
        )
        .route("/instructions", get(instructions_manage::list))
        .route(
            "/instructions/{scope}/{file}",
            get(instructions_manage::detail),
        )
        .route(
            "/instructions/{scope}/{file}/{action}",
            get(instructions_manage::confirm).post(instructions_manage::run),
        )
        .route(
            "/instructions/claude-agents-md",
            post(instructions_manage::claude_option),
        )
        .route("/import/skill", post(import::bring_skill))
        .route("/import/skill/theirs", post(import::take_theirs))
        .route("/import/skills", post(import::bring_all_skills))
        .route("/import/note", post(import::bring_note))
        .route("/import/notes", post(import::bring_all_notes))
        .route("/import/hook", post(import::bring_hook))
        .route("/import/hooks", post(import::bring_all_hooks))
        .route("/import/keep", post(import::keep))
        .route("/import/compare", get(import::compare))
        .route("/import/compare-copies", get(tidy::compare_copies))
}

/// Builds the hub's router.
pub fn router(state: AppState, access: Access) -> Router {
    let access = Arc::new(access);
    Router::new()
        .route("/", get(overview))
        .merge(catalog_routes())
        .route("/memory", get(memory))
        .route("/memory/{name}", get(memory_detail))
        .route("/hooks", get(hooks))
        .route("/mcp", get(mcp))
        .route("/machines", get(machines))
        .route("/machines/list", get(machines_list))
        .route("/signin", get(edit::signin_page))
        .route("/setup/{token}", get(edit::setup_link))
        .route("/auth/enrol/start", post(edit::enrol_start))
        .route("/auth/enrol/finish", post(edit::enrol_finish))
        .route("/auth/signin/start", post(edit::signin_start))
        .route("/auth/signin/finish", post(edit::signin_finish))
        .route("/auth/signout", post(edit::signout))
        .route("/auth/confirm/start", post(edit::confirm_start))
        .route("/auth/confirm/finish", post(edit::confirm_finish))
        .route(
            "/settings/signout-everywhere",
            post(edit::signout_everywhere),
        )
        .route(
            "/settings",
            get(edit::settings_page).post(edit::settings_save),
        )
        .route("/settings/keys/{id}/remove", post(edit::remove_key))
        .route("/settings/tokens", post(edit::create_token))
        .route("/settings/tokens/{id}/remove", post(edit::remove_token))
        .route("/releases", get(distribute::releases_moved))
        .route("/send/start", post(distribute::send_start))
        .route("/send/finish", post(distribute::send_finish))
        .route("/send/review", post(distribute::review_build))
        .route("/send/review/{id}", get(distribute::review_page))
        .route("/machines/pair", post(distribute::pair_code))
        .route("/machines/pair/ssh", post(distribute::pair_ssh))
        .route("/machines/add", post(distribute::add_machine_ui))
        .route("/machines/forget", post(distribute::forget_machine_ui))
        .route("/machines/{id}/remove", post(distribute::unpair))
        .route("/install", get(install::script))
        .route("/install/{name}", get(install::binary))
        .route("/api/pair", post(distribute::api_pair))
        .route("/api/release", get(distribute::api_release))
        .route("/api/report", post(distribute::api_report))
        .route(
            "/api/admin/machines",
            get(distribute::api_admin_machines).post(distribute::api_admin_add_machine),
        )
        .route(
            "/api/admin/machines/{target}",
            delete(distribute::api_admin_remove_machine),
        )
        .route(
            "/api/admin/pair-code",
            post(distribute::api_admin_pair_code),
        )
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
img-src 'self' data:; connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

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
    /// Send can deliver something (offered only then).
    can_send: bool,
}

#[derive(Template)]
#[template(path = "skills.html")]
struct SkillsPage {
    page: Page,
    v: views::SkillsList,
    outside: import::SkillsOutside,
    place: skills_place::ListPlace,
}

#[derive(Template)]
#[template(path = "skill.html")]
struct SkillPage {
    page: Page,
    v: views::SkillDetail,
    files: library_pages::FileTree,
    /// Who gets the skill: the switches and "Use in…".
    p: skills_place::SkillPlace,
    /// Its Check for updates page, for a skill installed from GitHub.
    update_href: Option<String>,
    /// Where this machine's agents differ from the switches (`machine_differs`).
    differs: Vec<String>,
}

#[derive(Template)]
#[template(path = "memory.html")]
struct MemoryPage {
    page: Page,
    v: views::MemoryList,
    outside: import::MemoryOutside,
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
    outside: import::HooksOutside,
}

/// The check before sending an update that adds or changes hooks.
#[derive(Template)]
#[template(path = "send_review.html")]
pub(crate) struct SendReviewPage {
    pub page: Page,
    /// The pending update this page shows.
    pub id: String,
    pub next: String,
    pub back: String,
    pub hooks: Vec<review::HookChange>,
    pub keys: Vec<review::KeyChange>,
    pub trusted: Vec<String>,
    pub others: Option<views::OthersView>,
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
    pair_target: String,
    ssh_result: Option<SshResult>,
}

/// The machine ledgers alone, polled by the Machines page.
#[derive(Template)]
#[template(path = "machines_list.html")]
struct MachinesList {
    page: Page,
    machines: Vec<views::Machine>,
}

/// What an install over SSH from the Machines page came to.
pub(crate) struct SshResult {
    pub ok: bool,
    pub message: String,
    pub output: String,
    /// The address typed, offered again after a failure so it needn't be retyped.
    pub target: String,
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
    /// After bringing something into Atlas: what to add to the notice (`import::done_text`).
    #[serde(default)]
    done: String,
    /// After a skill's switch or "Use in…": what changed (`turned` and `agent`, or `used`).
    #[serde(default)]
    turned: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    used: String,
    /// The Skills list row to open, as `<home>/<name>`.
    #[serde(default)]
    open: String,
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
        if let Some(did) = skills_place::did_text(&self.turned, &self.agent, &self.used) {
            if self.sent.is_some() {
                return (format!("{did} · on your machines within 10 minutes"), false);
            }
            if self.notsent.is_some() {
                return (format!("{did}. Not on your machines yet"), true);
            }
            return (did, false);
        }
        let saved = !self.saved.is_empty() && self.saved.bytes().all(|b| b.is_ascii_hexdigit());
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

    fn apply(&self, page: &mut Page, s: &AppState) {
        let (mut text, send) = self.flash();
        if let Some(extra) = import::done_text(&self.done) {
            text = if text.is_empty() {
                extra
            } else {
                format!("{text} {extra}")
            };
        }
        page.flash = text;
        page.flash_action = send.then_some("Send");
        if self.sent.is_some() {
            page.flash_more = distribute::sent_others(s);
        }
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
    q.apply(&mut page, &s);
    let waiting = distribute::waiting(&s).await;
    let can_send = waiting.as_ref().is_some_and(distribute::Waiting::sendable);
    let waiting = waiting.and_then(|w| w.text());
    Ok(Html(
        OverviewPage {
            page,
            v: views::overview(&snap),
            waiting,
            can_send,
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
    q.apply(&mut page, &s);
    let outside = import::skills_outside(&snap, &scope, &q.q);
    let place = skills_place::list(&s, &snap, &scope, &page, &q.open);
    Ok(Html(
        SkillsPage {
            page,
            v,
            outside,
            place,
        }
        .render()?,
    ))
}

async fn skill_detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::skill_detail(&snap, &home, &name).ok_or(AppError::NotFound)?;
    let skill = snap
        .catalog
        .skills
        .iter()
        .find(|s| s.home.key() == home && s.name.as_str() == name)
        .ok_or(AppError::NotFound)?;
    // A link from a project's list carries it, so "This project only" means that one.
    let scope = views::resolve_scope(&snap, if q.scope.is_empty() { &home } else { &q.scope });
    let mut page = s.page(&headers, "/skills", v.name.clone(), &snap, &scope, false);
    q.apply(&mut page, &s);
    page.edit_href = s
        .writer
        .is_some()
        .then(|| format!("/skills/{home}/{name}/edit"));
    let files = library_pages::file_tree(&snap, &home, &name, page.signed_in);
    let p = skills_place::detail(&s, &snap, skill, &scope, &page);
    // Only a skill installed from GitHub has anything to check.
    let update_href = (s.writer.is_some()
        && snap
            .library
            .source(&skill.home, skill.name.as_str())
            .is_some())
    .then(|| format!("/skills/{home}/{name}/update"));
    let differs = skills_place::machine_differs(&p.switches, &v.agents, snap.delivered);
    Ok(Html(
        SkillPage {
            page,
            v,
            files,
            p,
            update_href,
            differs,
        }
        .render()?,
    ))
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
    q.apply(&mut page, &s);
    let outside = import::memory_outside(&snap, &scope, &s.sources.home);
    Ok(Html(MemoryPage { page, v, outside }.render()?))
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
    q.apply(&mut page, &s);
    // Agents' own per-repo notes aren't in git, so only store notes can be edited.
    page.edit_href =
        (s.writer.is_some() && scope == Scope::Global).then(|| format!("/memory/{name}/edit"));
    Ok(Html(MemoryDetailPage { page, v }.render()?))
}

async fn hooks(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::hooks(&snap);
    let mut page = s.page(&headers, "/hooks", "Hooks", &snap, &Scope::Global, false);
    page.totals = format!("{} in Atlas · {} outside", v.atlas.len(), v.found.len());
    q.apply(&mut page, &s);
    let outside = import::hooks_outside(&snap);
    Ok(Html(HooksPage { page, v, outside }.render()?))
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
    machines_page(&s, &headers, None, None, None).await
}

/// The machine list, polled every 10 seconds so a newly paired machine appears.
async fn machines_list(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let (page, machines) = machines_data(&s, &headers).await?;
    Ok(Html(MachinesList { page, machines }.render()?).into_response())
}

/// The page frame and the machines, for the Machines page and its polled list.
async fn machines_data(
    s: &AppState,
    headers: &HeaderMap,
) -> Result<(Page, Vec<views::Machine>), AppError> {
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
    let others = s.known_machines();
    let machines = views::machines(&snap, &s.hub, &others, &clients, latest);
    let paired = machines
        .iter()
        .filter(|m| m.hub || !m.clients.is_empty())
        .count();
    page.totals = format!("{} machines · {paired} reporting", machines.len());
    Ok((page, machines))
}

/// The Machines page; `pair_command` shows a fresh pairing command once, `pair_target`
/// optionally names the machine for the SSH variant, `ssh_result` how an install over SSH went.
pub(crate) async fn machines_page(
    s: &AppState,
    headers: &HeaderMap,
    pair_command: Option<String>,
    pair_target: Option<String>,
    ssh_result: Option<SshResult>,
) -> Result<Response, AppError> {
    let (page, machines) = machines_data(s, headers).await?;
    let pair_target = pair_target.unwrap_or_else(|| "you@machine".to_owned());
    Ok(Html(
        MachinesPage {
            page,
            machines,
            pair_command,
            pair_target,
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
        "icon-192.png",
        "image/png",
        include_bytes!("../static/icon-192.png"),
    ),
    (
        "icon-512.png",
        "image/png",
        include_bytes!("../static/icon-512.png"),
    ),
    (
        "manifest.webmanifest",
        "application/manifest+json",
        include_bytes!("../static/manifest.webmanifest"),
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
