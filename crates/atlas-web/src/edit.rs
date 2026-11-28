//! Phase 2: passkey sign-in, settings, and editing skills, memory and instructions.
//!
//! Every save is one git commit of one file (strategy: nothing is lost). Writes need a
//! passkey session, its CSRF token and a same-origin request.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use askama::Template;
use atlas_core::catalog::SkillHome;
use atlas_core::catalog::edit::{EditTarget, RepoKind};
use atlas_core::kernel::{Scope, Slug};
use atlas_fs::edit::{EditError, Located};
use atlas_signer::{PublicKeyCredential, RegisterPublicKeyCredential, Signer};
use axum::Json;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};

use crate::auth::{self, Auth, Flow};
use crate::views::Page;
use crate::{AppError, AppState};

/// The owner's settings, kept in the hub's state folder.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Push project repos after each saved edit. Knowledge is pushed by sending; the public
    /// skills repo never is.
    #[serde(default)]
    pub push_after_save: bool,
    /// Saving also sends the change to the owner's machines (one passkey tap).
    #[serde(default = "yes")]
    pub send_on_save: bool,
}

const fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            push_after_save: false,
            send_on_save: true,
        }
    }
}

/// Everything writes need. Absent when the hub has no public address configured, in
/// which case Atlas stays read-only.
pub struct Writer {
    pub signer: Signer,
    pub auth: Auth,
    /// The hub's own origin, such as `https://hub.example.ts.net:7443`.
    pub origin: String,
    settings_path: PathBuf,
    settings: Mutex<Settings>,
    /// One save (or push before sending) at a time: git can't take two at once.
    pub(crate) git: Mutex<()>,
    /// Passkeys, setup links and settings.
    pub state_dir: PathBuf,
    started: std::time::Instant,
    /// How long after start the first passkey can be made without a setup link.
    first_run: std::time::Duration,
    /// Paired machines and their clients' tokens.
    pub clients: crate::clients::Clients,
    /// Sent updates (signed releases).
    pub releases: crate::release::Store,
    /// An update built and waiting for the owner's signature.
    pub pending: Mutex<Option<crate::distribute::Pending>>,
}

impl std::fmt::Debug for Writer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Writer")
            .field("origin", &self.origin)
            .finish_non_exhaustive()
    }
}

impl Writer {
    /// Opens the passkey store and settings in `state_dir` for a hub served at `public_url`.
    /// While no passkey exists, the first one can be made without a setup link for
    /// `first_run` after start, by an allowed Tailscale login.
    ///
    /// # Errors
    /// If the address isn't a usable https origin or the state can't be read.
    pub fn open(
        public_url: &str,
        state_dir: &std::path::Path,
        first_run: std::time::Duration,
    ) -> Result<Self, String> {
        let url = atlas_signer::Url::parse(public_url).map_err(|e| e.to_string())?;
        // Browsers allow passkeys only on https, or on plain http for localhost (tests).
        let local = url.scheme() == "http" && url.host_str() == Some("localhost");
        if url.scheme() != "https" && !local {
            return Err("public_url must be https (passkeys need it)".to_owned());
        }
        let signer = Signer::open(&url, state_dir).map_err(|e| e.to_string())?;
        let settings_path = state_dir.join("settings.json");
        let settings = std::fs::read(&settings_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Ok(Self {
            signer,
            auth: Auth::new(),
            origin: url.origin().ascii_serialization(),
            settings_path,
            settings: Mutex::new(settings),
            git: Mutex::new(()),
            state_dir: state_dir.to_path_buf(),
            started: std::time::Instant::now(),
            first_run,
            clients: crate::clients::Clients::open(state_dir),
            releases: crate::release::Store::new(state_dir),
            pending: Mutex::new(None),
        })
    }

    /// The public address, for links printed elsewhere.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Whether the first passkey can still be made without a setup link.
    #[must_use]
    pub fn first_run_open(&self) -> bool {
        !self.signer.has_keys() && self.started.elapsed() < self.first_run
    }

    /// Whole minutes left in the first-run window, rounded up.
    fn first_run_minutes_left(&self) -> u64 {
        self.first_run
            .saturating_sub(self.started.elapsed())
            .as_secs()
            .div_ceil(60)
    }

    pub(crate) fn settings(&self) -> Settings {
        self.settings
            .lock()
            .map_or_else(|_| Settings::default(), |s| s.clone())
    }

    fn save_settings(&self, new: Settings) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(&new).map_err(|e| e.to_string())?;
        let tmp = self.settings_path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp, &self.settings_path)
        };
        write().map_err(|e| e.to_string())?;
        if let Ok(mut s) = self.settings.lock() {
            *s = new;
        }
        Ok(())
    }

    /// A fresh signed-in session for the crate's HTTP tests, as `(cookie header, CSRF
    /// token)`. Only with the `testing` feature, which the hub binary never enables.
    #[cfg(feature = "testing")]
    #[doc(hidden)]
    #[must_use]
    pub fn test_session(&self) -> (String, String) {
        let token = self.auth.sign_in();
        let cookie = format!("{}={token}", auth::COOKIE);
        let mut h = HeaderMap::new();
        if let Ok(v) = cookie.parse() {
            h.insert(axum::http::header::COOKIE, v);
        }
        let csrf = self.auth.session(&h).map_or_else(String::new, |s| s.csrf);
        (cookie, csrf)
    }

    /// The live session, if this is a same-origin request carrying the session's token.
    pub(crate) fn authorised(&self, headers: &HeaderMap, csrf: &str) -> bool {
        auth::same_origin(headers, &self.origin)
            && self
                .auth
                .session(headers)
                .is_some_and(|s| auth::same_secret(&s.csrf, csrf))
    }
}

/// Adds sign-in state to a page's frame.
pub(crate) fn decorate(page: &mut Page, state: &AppState, headers: &HeaderMap) {
    if let Some(w) = &state.writer {
        page.can_sign_in = true;
        if let Some(s) = w.auth.session(headers) {
            page.signed_in = true;
            page.csrf = s.csrf;
        }
    }
}

pub(crate) fn writer(state: &AppState) -> Result<&Arc<Writer>, AppError> {
    state.writer.as_ref().ok_or(AppError::NotFound)
}

pub(crate) fn forbidden(msg: &'static str) -> Response {
    (StatusCode::FORBIDDEN, msg).into_response()
}

// ---------------------------------------------------------------- sign-in pages

#[derive(Template)]
#[template(path = "signin.html")]
struct SignInPage {
    page: Page,
    mode: SignInMode,
    next: String,
    /// The Tailscale login on first run.
    login: String,
    minutes_left: u64,
    /// The setup link's token.
    token: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SignInMode {
    /// No passkey yet, and the first-run window is open.
    FirstRun,
    /// No passkey yet, and the window has closed.
    Closed,
    /// A setup link from `atlas-hub setup`.
    SetupLink,
    /// The usual passkey sign-in.
    SignIn,
}

#[derive(Debug, Default, Deserialize)]
pub struct NextQuery {
    #[serde(default)]
    next: String,
}

pub(crate) async fn signin_page(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<NextQuery>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    let next = auth::safe_next(&q.next);
    if w.auth.session(&headers).is_some() {
        return Ok(Redirect::to(&next).into_response());
    }
    let mode = if w.signer.has_keys() {
        SignInMode::SignIn
    } else if w.first_run_open() {
        SignInMode::FirstRun
    } else {
        SignInMode::Closed
    };
    let title = if mode == SignInMode::SignIn {
        "Sign in"
    } else {
        "Welcome to Atlas"
    };
    render_signin(&s, &headers, mode, title, next, String::new()).await
}

async fn render_signin(
    s: &AppState,
    headers: &HeaderMap,
    mode: SignInMode,
    title: &str,
    next: String,
    token: String,
) -> Result<Response, AppError> {
    let w = writer(s)?;
    let snap = s.snapshot().await?;
    let page = s.page(headers, "/signin", title, &snap, &Scope::Global, false);
    Ok(Html(
        SignInPage {
            page,
            mode,
            next,
            login: login(headers),
            minutes_left: w.first_run_minutes_left(),
            token,
        }
        .render()?,
    )
    .into_response())
}

/// A setup link from `atlas-hub setup`: one passkey, no sign-in needed.
pub(crate) async fn setup_link(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(token): Path<String>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !atlas_signer::setup::is_valid(&w.state_dir, &token) {
        return Ok((
            StatusCode::NOT_FOUND,
            "This setup link has expired or was already used. On the hub, run: atlas-hub setup\n",
        )
            .into_response());
    }
    render_signin(
        &s,
        &headers,
        SignInMode::SetupLink,
        "Add a passkey",
        "/settings".into(),
        token,
    )
    .await
}

#[derive(Deserialize)]
pub struct EnrolStart {
    #[serde(default)]
    name: String,
    /// A setup link's token, when enrolling through one.
    #[serde(default)]
    token: String,
}

#[derive(Serialize)]
struct Started<T: Serialize> {
    flow: String,
    options: T,
}

pub(crate) async fn enrol_start(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<EnrolStart>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !auth::same_origin(&headers, &w.origin) {
        return Ok(forbidden("Wrong origin.\n"));
    }
    let token = Some(req.token).filter(|t| !t.is_empty());
    let added_by = if let Some(t) = &token {
        // A setup link from `atlas-hub setup`: whoever ran it controls the hub.
        if !atlas_signer::setup::is_valid(&w.state_dir, t) {
            return Ok(forbidden(
                "This setup link has expired or was already used. Run atlas-hub setup for a new one.\n",
            ));
        }
        "setup link".to_owned()
    } else if w.signer.has_keys() {
        // Adding another passkey needs a session and its token.
        let csrf = headers
            .get("x-atlas-csrf")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !w.authorised(&headers, csrf) {
            return Ok(forbidden("Sign in to add another passkey.\n"));
        }
        "signed in".to_owned()
    } else if w.first_run_open() {
        // First run: Tailscale already told us who this is (the login check ran first).
        login(&headers)
    } else {
        return Ok(forbidden(
            "The first-run window has closed. On the hub, run atlas-hub setup for a link.\n",
        ));
    };
    let (options, state) = w
        .signer
        .start_enrol()
        .map_err(|e| AppError::Bad(e.to_string()))?;
    let flow = w.auth.start_flow(Flow::Enrol {
        state: Box::new(state),
        name: req.name,
        added_by,
        token,
    });
    Ok(Json(Started { flow, options }).into_response())
}

/// The Tailscale login `tailscale serve` put on this request.
fn login(headers: &HeaderMap) -> String {
    headers
        .get("tailscale-user-login")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown login")
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect()
}

#[derive(Deserialize)]
pub struct EnrolFinish {
    flow: String,
    credential: RegisterPublicKeyCredential,
}

#[derive(Serialize)]
struct Done {
    next: String,
}

pub(crate) async fn enrol_finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<EnrolFinish>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !auth::same_origin(&headers, &w.origin) {
        return Ok(forbidden("Wrong origin.\n"));
    }
    let Some(Flow::Enrol {
        state,
        name,
        added_by,
        token,
    }) = w.auth.take_flow(&req.flow)
    else {
        return Ok(forbidden("That passkey request expired. Try again.\n"));
    };
    let first = !w.signer.has_keys();
    // Check again at the finish: first run can be claimed once, and a link used once.
    let still_allowed = match &token {
        Some(t) => atlas_signer::setup::redeem(&w.state_dir, t)
            .map_err(|e| AppError::Bad(e.to_string()))?,
        None => !first || w.first_run_open(),
    };
    if !still_allowed {
        return Ok(forbidden(
            "This setup has already been used or has expired.\n",
        ));
    }
    let key = w
        .signer
        .finish_enrol(&req.credential, &state, &name, &added_by)
        .map_err(|e| AppError::Bad(e.to_string()))?;
    tracing::warn!("passkey \"{}\" enrolled ({added_by})", key.name);
    let mut res = Json(Done {
        next: "/settings".into(),
    })
    .into_response();
    if first || w.auth.session(&headers).is_none() {
        let id = w.auth.sign_in();
        res.headers_mut()
            .insert(header::SET_COOKIE, auth::set_cookie(&id));
    }
    Ok(res)
}

pub(crate) async fn signin_start(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(q): Json<NextQuery>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !auth::same_origin(&headers, &w.origin) {
        return Ok(forbidden("Wrong origin.\n"));
    }
    let Some((options, state)) = w
        .signer
        .start_sign_in()
        .map_err(|e| AppError::Bad(e.to_string()))?
    else {
        return Ok(forbidden("No passkey yet. Add one with the setup code.\n"));
    };
    let flow = w.auth.start_flow(Flow::SignIn {
        state: Box::new(state),
        next: auth::safe_next(&q.next),
    });
    Ok(Json(Started { flow, options }).into_response())
}

#[derive(Deserialize)]
pub struct SignInFinish {
    flow: String,
    credential: PublicKeyCredential,
}

pub(crate) async fn signin_finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SignInFinish>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !auth::same_origin(&headers, &w.origin) {
        return Ok(forbidden("Wrong origin.\n"));
    }
    let Some(Flow::SignIn { state, next }) = w.auth.take_flow(&req.flow) else {
        return Ok(forbidden("That sign-in expired. Try again.\n"));
    };
    if w.signer.finish_sign_in(&req.credential, &state).is_err() {
        return Ok(forbidden("That passkey didn't check out.\n"));
    }
    let id = w.auth.sign_in();
    let mut res = Json(Done { next }).into_response();
    res.headers_mut()
        .insert(header::SET_COOKIE, auth::set_cookie(&id));
    Ok(res)
}

#[derive(Deserialize)]
pub struct CsrfForm {
    #[serde(default)]
    csrf: String,
}

impl CsrfForm {
    pub(crate) fn csrf(&self) -> &str {
        &self.csrf
    }
}

pub(crate) async fn signout(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Not signed in.\n"));
    }
    w.auth.sign_out(&headers);
    let mut res = Redirect::to("/").into_response();
    res.headers_mut()
        .insert(header::SET_COOKIE, auth::clear_cookie());
    Ok(res)
}

// ---------------------------------------------------------------- settings

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    page: Page,
    keys: Vec<KeyRow>,
    push_after_save: bool,
    send_on_save: bool,
    /// "3 changes aren't on your machines yet", when some are waiting.
    waiting: Option<String>,
    updates: Vec<crate::distribute::UpdateRow>,
    flash: String,
}

struct KeyRow {
    id: String,
    name: String,
    added: String,
    added_by: String,
    last_used: String,
}

/// Days since the epoch to a civil date (Howard Hinnant's algorithm).
fn civil(secs: u64) -> (i64, i64, i64) {
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub(crate) fn day(secs: u64) -> String {
    let (y, m, d) = civil(secs);
    format!("{y}-{m:02}-{d:02}")
}

/// A short date such as "27 Nov".
pub(crate) fn day_month(secs: u64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = civil(secs);
    let month = usize::try_from(m - 1).map_or("", |i| MONTHS.get(i).copied().unwrap_or(""));
    format!("{d} {month}")
}

#[derive(Debug, Default, Deserialize)]
pub struct FlashQuery {
    #[serde(default)]
    flash: String,
}

pub(crate) async fn settings_page(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<FlashQuery>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if w.auth.session(&headers).is_none() {
        return Ok(Redirect::to("/signin?next=/settings").into_response());
    }
    let snap = s.snapshot().await?;
    let page = s.page(
        &headers,
        "/settings",
        "Settings",
        &snap,
        &Scope::Global,
        false,
    );
    let keys = w
        .signer
        .keys()
        .into_iter()
        .map(|k| KeyRow {
            id: k.id,
            name: k.name,
            added: day(k.added),
            added_by: k.added_by.unwrap_or_default(),
            last_used: k.last_used.map_or_else(|| "never".to_owned(), day),
        })
        .collect();
    let flash = match q.flash.as_str() {
        "saved" => "Settings saved.",
        "removed" => "Passkey removed.",
        "last" => "That's your only passkey, so it stays.",
        "sent" => "On your machines within 10 minutes",
        _ => "",
    };
    let settings = w.settings();
    let waiting = crate::distribute::waiting(&s).await.and_then(|w| w.text());
    Ok(Html(
        SettingsPage {
            page,
            keys,
            push_after_save: settings.push_after_save,
            send_on_save: settings.send_on_save,
            waiting,
            updates: crate::distribute::history(w),
            flash: flash.to_owned(),
        }
        .render()?,
    )
    .into_response())
}

#[derive(Deserialize)]
pub struct SettingsForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    push_after_save: Option<String>,
    #[serde(default)]
    send_on_save: Option<String>,
}

pub(crate) async fn settings_save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<SettingsForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to change settings.\n"));
    }
    w.save_settings(Settings {
        push_after_save: f.push_after_save.is_some(),
        send_on_save: f.send_on_save.is_some(),
    })
    .map_err(AppError::Bad)?;
    Ok(Redirect::to("/settings?flash=saved").into_response())
}

pub(crate) async fn remove_key(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to remove a passkey.\n"));
    }
    let flash = match w.signer.remove(&id) {
        Ok(()) => "removed",
        Err(atlas_signer::SignerError::LastKey) => "last",
        Err(e) => return Err(AppError::Bad(e.to_string())),
    };
    Ok(Redirect::to(&format!("/settings?flash={flash}")).into_response())
}

// ---------------------------------------------------------------- editing

#[derive(Template)]
#[template(path = "edit.html")]
struct EditPage {
    page: Page,
    shown: String,
    text: String,
    base: String,
    back: String,
    error: String,
    note: String,
    /// Saving also sends the change (the page's script asks for the passkey).
    send_on_save: bool,
}

/// Where an edit's form lives and where to go after saving.
struct Item {
    target: EditTarget,
    title: String,
    section: &'static str,
    view: String,
    edit: String,
    label: String,
}

fn skill_item(home: &str, name: &str, snap_projects: &[Slug]) -> Option<Item> {
    let slug = Slug::new(name).ok()?;
    let home_kind = match home {
        "private" => SkillHome::Private,
        "public" => SkillHome::Public,
        p => SkillHome::Project(snap_projects.iter().find(|s| s.as_str() == p)?.clone()),
    };
    Some(Item {
        target: EditTarget::Skill {
            home: home_kind,
            name: slug,
        },
        title: name.to_owned(),
        section: "/skills",
        view: format!("/skills/{home}/{name}"),
        edit: format!("/skills/{home}/{name}/edit"),
        label: format!("skill {name}"),
    })
}

/// A store memory note, found by name in the catalog (which knows its file).
fn memory_item(snap: &crate::views::Snapshot, name: &str) -> Option<Item> {
    let page = snap
        .catalog
        .memory
        .iter()
        .find(|m| m.name == name && m.scope == Scope::Global && !m.file.is_empty())?;
    Some(Item {
        target: EditTarget::Memory {
            name: name.to_owned(),
            file: page.file.clone(),
        },
        title: name.to_owned(),
        section: "/memory",
        view: format!("/memory/{name}"),
        edit: format!("/memory/{name}/edit"),
        label: format!("memory {name}"),
    })
}

fn instruction_item(scope: &str, file: &str, projects: &[Slug]) -> Option<Item> {
    let scope_v = if scope == "global" {
        Scope::Global
    } else {
        Scope::Project(projects.iter().find(|s| s.as_str() == scope)?.clone())
    };
    Some(Item {
        target: EditTarget::Instructions {
            scope: scope_v,
            file: file.to_owned(),
        },
        title: if scope == "global" {
            format!("Global {file}")
        } else {
            format!("{scope} · {file}")
        },
        section: "/instructions",
        view: format!("/instructions/{scope}/{file}"),
        edit: format!("/instructions/{scope}/{file}/edit"),
        label: format!(
            "{} {file}",
            if scope == "global" { "global" } else { scope }
        ),
    })
}

fn projects(s: &AppState) -> Vec<Slug> {
    s.sources.projects.iter().map(|p| p.name.clone()).collect()
}

fn note_for(kind: &RepoKind, settings: &Settings) -> String {
    match kind {
        RepoKind::PublicSkills => "Saving commits to the public skills repo. Atlas never pushes it: check it for private details, then push by hand.".to_owned(),
        RepoKind::Project(p) => format!("Saving commits to {p}'s main branch{}.", if kind.may_push(settings.push_after_save) { " and pushes it, which may deploy it" } else { "" }),
        RepoKind::Knowledge => format!("Saving commits to the knowledge repo{}.", if settings.send_on_save { " and sends it to your machines" } else { "" }),
    }
}

async fn show_edit(s: &AppState, headers: &HeaderMap, item: Item) -> Result<Response, AppError> {
    let w = writer(s)?;
    if w.auth.session(headers).is_none() {
        return Ok(Redirect::to(&format!("/signin?next={}", item.edit)).into_response());
    }
    let sources = Arc::clone(&s.sources);
    let target = item.target.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let located = atlas_fs::edit::locate(&sources, &target).ok_or(EditError::NotFound)?;
        let text = atlas_fs::edit::read_raw(&located)?;
        Ok::<_, EditError>((located, text))
    })
    .await
    .map_err(|_| AppError::Internal)?;
    let (located, text) = match loaded {
        Ok(v) => v,
        Err(EditError::NotFound) => return Err(AppError::NotFound),
        Err(e) => return Err(AppError::Bad(e.to_string())),
    };
    render_edit(
        s,
        headers,
        &item,
        &located,
        &text,
        &atlas_fs::edit::content_hash(&text),
        String::new(),
    )
    .await
}

async fn render_edit(
    s: &AppState,
    headers: &HeaderMap,
    item: &Item,
    located: &Located,
    text: &str,
    base: &str,
    error: String,
) -> Result<Response, AppError> {
    let w = writer(s)?;
    let snap = s.snapshot().await?;
    let page = s.page(
        headers,
        item.section,
        format!("Edit {}", item.title),
        &snap,
        &Scope::Global,
        false,
    );
    let settings = w.settings();
    let note = note_for(&located.kind, &settings);
    let send_on_save = settings.send_on_save && located.kind == RepoKind::Knowledge;
    let status = if error.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    Ok((
        status,
        Html(
            EditPage {
                page,
                shown: located.shown.clone(),
                text: text.to_owned(),
                base: base.to_owned(),
                back: item.view.clone(),
                error,
                note,
                send_on_save,
            }
            .render()?,
        ),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct EditForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    text: String,
}

/// What happened to a save, for the page it returns to.
enum Saved {
    Committed {
        sha: String,
        pushed: Option<bool>,
        kind: RepoKind,
    },
    Unchanged,
}

/// A save's answer to the page's script: where to go, and whether to send first.
#[derive(Serialize)]
struct SavedJson {
    next: String,
    send: bool,
}

pub(crate) fn wants_json(headers: &HeaderMap) -> bool {
    headers
        .get(header::ACCEPT)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("application/json"))
}

async fn save(
    s: &AppState,
    headers: &HeaderMap,
    item: Item,
    form: EditForm,
) -> Result<Response, AppError> {
    let w = Arc::clone(writer(s)?);
    if !w.authorised(headers, &form.csrf) {
        tracing::warn!(
            "save refused: origin {:?}, signed in {}",
            headers.get(header::ORIGIN),
            w.auth.session(headers).is_some()
        );
        return Ok(forbidden(
            "Your session ended. Sign in again; your text wasn't saved.\n",
        ));
    }
    let sources = Arc::clone(&s.sources);
    let target = item.target.clone();
    let settings = w.settings();
    let push_setting = settings.push_after_save;
    let push_setting_send = settings.send_on_save;
    let label = item.label.clone();
    let text = form.text.clone();
    let base = form.base.clone();
    let writer = Arc::clone(&w);
    let outcome = tokio::task::spawn_blocking(move || -> Result<Saved, Box<(String, Option<Located>)>> {
        let located = atlas_fs::edit::locate(&sources, &target).ok_or_else(|| Box::new(("That item no longer exists.".to_owned(), None)))?;
        let fail = |msg: String| Box::new((msg, Some(located.clone())));
        let prepared = atlas_core::catalog::edit::prepare(&text, located.markdown).map_err(|e| fail(e.to_string()))?;
        let _guard = writer.git.lock().map_err(|_| fail("Another save is stuck.".to_owned()))?;
        let repo = atlas_git::Repo::open(&located.root).map_err(|e| fail(e.to_string()))?;
        if let Some(branch) = located.kind.required_branch() {
            let actual = repo.current_branch().map_err(|e| fail(e.to_string()))?;
            if actual.as_deref() != Some(branch) {
                return Err(fail(format!(
                    "{} is on {}, not {branch}. Edits commit to {branch}; switch branch first.",
                    located.root.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
                    actual.unwrap_or_else(|| "no branch".to_owned())
                )));
            }
        }
        let rel = std::path::Path::new(&located.rel);
        repo.ensure_clean(rel).map_err(|_| fail("This file has uncommitted changes made outside Atlas. Commit or discard them first.".to_owned()))?;
        let before = match atlas_fs::edit::write_if_unchanged(&located, &base, &prepared) {
            Ok(before) => before,
            Err(EditError::Changed) => return Err(fail("The file changed since you opened it. Your text is below; copy what you need and reload.".to_owned())),
            Err(e) => return Err(fail(e.to_string())),
        };
        // The branch was checked just above, under the save lock; no need to ask git twice.
        let sha = match repo.commit_file(rel, &format!("Edit {label} in Atlas"), None) {
            Ok(sha) => sha,
            Err(atlas_git::GitError::NothingToCommit) => return Ok(Saved::Unchanged),
            Err(e) => {
                // Nothing is lost: put the old text back so the file matches git again.
                let _ = atlas_fs::edit::write_raw(&located, &before);
                let text = e.to_string();
                let msg = if text.contains("tell me who you are") || text.contains("empty ident") {
                    "This repo has no git name and email set, so git won't commit. Set user.name and user.email in it, then save again. Nothing changed.".to_owned()
                } else {
                    format!("The commit failed, so nothing changed: {text}")
                };
                return Err(fail(msg));
            }
        };
        let pushed = located.kind.may_push(push_setting).then(|| {
            repo.push().map_err(|e| tracing::warn!("push after save failed: {e}")).is_ok()
        });
        Ok(Saved::Committed { sha, pushed, kind: located.kind.clone() })
    })
    .await
    .map_err(|_| AppError::Internal)?;
    // Whatever happened, the next page should read the store as it now is.
    s.invalidate().await;

    let json = wants_json(headers);
    let done = |next: String, send: bool| {
        if json {
            Json(SavedJson { next, send }).into_response()
        } else {
            Redirect::to(&next).into_response()
        }
    };
    match outcome {
        Ok(Saved::Committed { sha, pushed, kind }) => {
            let push = match (pushed, &kind) {
                (Some(true), _) => "pushed",
                (Some(false), _) => "pushfailed",
                (None, RepoKind::PublicSkills) => "public",
                (None, _) => "committed",
            };
            let next = format!("{}?saved={}&{push}=1", item.view, &sha[..sha.len().min(10)]);
            let send = kind == RepoKind::Knowledge && push_setting_send;
            Ok(done(next, send))
        }
        Ok(Saved::Unchanged) => Ok(done(format!("{}?unchanged=1", item.view), false)),
        Err(failed) => match *failed {
            (msg, Some(located)) => {
                render_edit(s, headers, &item, &located, &form.text, &form.base, msg).await
            }
            (msg, None) => Err(AppError::Bad(msg)),
        },
    }
}

pub(crate) async fn edit_skill(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let item = skill_item(&home, &name, &projects(&s)).ok_or(AppError::NotFound)?;
    show_edit(&s, &headers, item).await
}

pub(crate) async fn save_skill(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name)): Path<(String, String)>,
    Form(f): Form<EditForm>,
) -> Result<Response, AppError> {
    let item = skill_item(&home, &name, &projects(&s)).ok_or(AppError::NotFound)?;
    save(&s, &headers, item, f).await
}

pub(crate) async fn edit_memory(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let item = memory_item(&snap, &name).ok_or(AppError::NotFound)?;
    show_edit(&s, &headers, item).await
}

pub(crate) async fn save_memory(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(name): Path<String>,
    Form(f): Form<EditForm>,
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let item = memory_item(&snap, &name).ok_or(AppError::NotFound)?;
    save(&s, &headers, item, f).await
}

pub(crate) async fn edit_instructions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((scope, file)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let item = instruction_item(&scope, &file, &projects(&s)).ok_or(AppError::NotFound)?;
    show_edit(&s, &headers, item).await
}

pub(crate) async fn save_instructions(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((scope, file)): Path<(String, String)>,
    Form(f): Form<EditForm>,
) -> Result<Response, AppError> {
    let item = instruction_item(&scope, &file, &projects(&s)).ok_or(AppError::NotFound)?;
    save(&s, &headers, item, f).await
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::process::Command;

    use atlas_core::kernel::Slug;
    use atlas_fs::{Project, Sources};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    use crate::{Access, AppState, router};

    const ORIGIN: &str = "https://hub.test";
    const LOGIN: &str = "owner@github";

    #[test]
    fn days_are_civil_dates() {
        assert_eq!(super::day(0), "1970-01-01");
        assert_eq!(super::day(1_795_737_600), "2026-11-27");
    }

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
        state: std::path::PathBuf,
        _tmp: tempfile::TempDir,
        knowledge: std::path::PathBuf,
        project: std::path::PathBuf,
        app: axum::Router,
        cookie: String,
        csrf: String,
    }

    fn hub() -> Hub {
        hub_with(std::time::Duration::from_mins(30))
    }

    fn hub_with(first_run: std::time::Duration) -> Hub {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let knowledge = home.join("Knowledge");
        repo(
            &knowledge,
            &[
                ("AGENTS.md", "# Rules\n"),
                (
                    "memory/fact.md",
                    "---\nname: fact\ndescription: A fact\nmetadata:\n  type: user\n---\nOld body.\n",
                ),
                ("memory/other.md", "---\nname: other\n---\nUntouched.\n"),
            ],
        );
        let project = home.join("Projects/shop");
        repo(
            &project,
            &[
                ("AGENTS.md", "# Shop\n"),
                (
                    ".agents/skills/stock/SKILL.md",
                    "---\ndescription: Count\n---\nBody\n",
                ),
            ],
        );
        let sources = Sources {
            home: home.clone(),
            knowledge: knowledge.clone(),
            public_skills: None,
            projects: vec![Project {
                name: Slug::new("shop").unwrap(),
                root: project.clone(),
            }],
        };
        let writer = super::Writer::open(ORIGIN, &home.join("state"), first_run).unwrap();
        let cookie = writer.auth.sign_in();
        let csrf = writer
            .auth
            .session(&{
                let mut h = axum::http::HeaderMap::new();
                h.insert(
                    header::COOKIE,
                    format!("{}={cookie}", crate::auth::COOKIE).parse().unwrap(),
                );
                h
            })
            .unwrap()
            .csrf;
        let state = AppState::new(sources, "test-hub", Vec::new(), Some(writer));
        let app = router(state, Access::new(vec![LOGIN.to_owned()]));
        Hub {
            state: home.join("state"),
            _tmp: tmp,
            knowledge,
            project,
            app,
            cookie,
            csrf,
        }
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
            req = req.header(
                header::COOKIE,
                format!("{}={}", crate::auth::COOKIE, hub.cookie),
            );
        }
        req.body(Body::empty()).unwrap()
    }

    fn post(
        hub: &Hub,
        path: &str,
        fields: &[(&str, &str)],
        origin: &str,
        signed_in: bool,
    ) -> Request<Body> {
        let body: String = fields
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencode(v)))
            .collect::<Vec<_>>()
            .join("&");
        let mut req = Request::post(path)
            .header("tailscale-user-login", LOGIN)
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        if signed_in {
            req = req.header(
                header::COOKIE,
                format!("{}={}", crate::auth::COOKIE, hub.cookie),
            );
        }
        req.body(Body::from(body)).unwrap()
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

    fn base_of(page: &str) -> String {
        let at = page.find(r#"name="base" value=""#).unwrap() + r#"name="base" value=""#.len();
        page[at..at + 32].to_owned()
    }

    #[tokio::test]
    async fn editing_needs_a_session() {
        let hub = hub();
        let (status, location, _) = send(&hub, get(&hub, "/memory/fact/edit", false)).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(location, "/signin?next=/memory/fact/edit");
        let (status, ..) = send(
            &hub,
            post(
                &hub,
                "/memory/fact/edit",
                &[("csrf", &hub.csrf), ("base", "x"), ("text", "x")],
                ORIGIN,
                false,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, ..) = send(
            &hub,
            post(
                &hub,
                "/memory/fact/edit",
                &[("csrf", &hub.csrf), ("base", "x"), ("text", "x")],
                "https://evil.test",
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "another origin");
        let (status, ..) = send(
            &hub,
            post(
                &hub,
                "/memory/fact/edit",
                &[("csrf", "wrong"), ("base", "x"), ("text", "x")],
                ORIGIN,
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN, "bad csrf");
        assert!(
            fs::read_to_string(hub.knowledge.join("memory/fact.md"))
                .unwrap()
                .contains("Old body.")
        );
    }

    #[tokio::test]
    async fn a_save_is_one_commit_of_one_file() {
        let hub = hub();
        fs::write(hub.knowledge.join("memory/other.md"), "dirty, not ours\n").unwrap();
        let (status, _, page) = send(&hub, get(&hub, "/memory/fact/edit", true)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("Old body."));
        let base = base_of(&page);
        let new = "---\r\nname: fact\r\ndescription: A fact\r\nmetadata:\r\n  type: user\r\n---\r\nNew body.\r\n";
        let (status, location, _) = send(
            &hub,
            post(
                &hub,
                "/memory/fact/edit",
                &[("csrf", &hub.csrf), ("base", &base), ("text", new)],
                ORIGIN,
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert!(location.starts_with("/memory/fact?saved="), "{location}");
        let on_disk = fs::read_to_string(hub.knowledge.join("memory/fact.md")).unwrap();
        assert!(on_disk.ends_with("New body.\n") && !on_disk.contains('\r'));
        assert_eq!(
            git(&hub.knowledge, &["log", "-1", "--format=%s"]).trim(),
            "Edit memory fact in Atlas"
        );
        assert_eq!(
            git(
                &hub.knowledge,
                &["show", "--name-only", "--format=", "HEAD"]
            )
            .trim(),
            "memory/fact.md"
        );
        assert_eq!(
            fs::read_to_string(hub.knowledge.join("memory/other.md")).unwrap(),
            "dirty, not ours\n"
        );
        let (_, _, view) = send(&hub, get(&hub, &location, true)).await;
        assert!(view.contains("Saved and committed as"));
        // The cached read of the store was dropped by the save, so the page is current.
        assert!(view.contains("New body."), "stale page after save");
    }

    #[tokio::test]
    async fn a_save_from_the_page_script_answers_where_to_go_and_whether_to_send() {
        let hub = hub();
        let (_, _, page) = send(&hub, get(&hub, "/memory/fact/edit", true)).await;
        assert!(
            page.contains("data-send-on-save"),
            "sending is on by default"
        );
        let base = base_of(&page);
        let mut req = post(
            &hub,
            "/memory/fact/edit",
            &[
                ("csrf", &hub.csrf),
                ("base", &base),
                ("text", "---\nname: fact\n---\nNew.\n"),
            ],
            ORIGIN,
            true,
        );
        req.headers_mut()
            .insert(header::ACCEPT, "application/json".parse().unwrap());
        let (status, _, body) = send(&hub, req).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let saved: serde_json::Value = serde_json::from_str(&body).unwrap();
        let sha = git(&hub.knowledge, &["rev-parse", "--short=10", "HEAD"]);
        assert_eq!(
            saved["next"].as_str().unwrap(),
            format!("/memory/fact?saved={}&committed=1", sha.trim())
        );
        assert_eq!(saved["send"], true);
        let (_, _, view) = send(
            &hub,
            get(
                &hub,
                &format!("/memory/fact?saved={}&notsent=1", sha.trim()),
                true,
            ),
        )
        .await;
        assert!(
            view.contains("Saved. Not on your machines yet")
                && view.contains(r#"data-action="send""#),
            "{view}"
        );
        let (_, _, view) = send(
            &hub,
            get(
                &hub,
                &format!("/memory/fact?saved={}&sent=1", sha.trim()),
                true,
            ),
        )
        .await;
        assert!(
            view.contains("Saved · on your machines within 10 minutes"),
            "{view}"
        );

        // Project repos aren't sent to machines, so their saves don't ask for a tap.
        let (_, _, page) = send(&hub, get(&hub, "/skills/shop/stock/edit", true)).await;
        assert!(!page.contains("data-send-on-save"));
        let mut req = post(
            &hub,
            "/skills/shop/stock/edit",
            &[
                ("csrf", &hub.csrf),
                ("base", &base_of(&page)),
                ("text", "---\ndescription: Count\n---\nNew\n"),
            ],
            ORIGIN,
            true,
        );
        req.headers_mut()
            .insert(header::ACCEPT, "application/json".parse().unwrap());
        let (status, _, body) = send(&hub, req).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains(r#""send":false"#), "{body}");

        // With sending off, the editor saves only.
        let (status, ..) = send(
            &hub,
            post(&hub, "/settings", &[("csrf", &hub.csrf)], ORIGIN, true),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (_, _, page) = send(&hub, get(&hub, "/memory/fact/edit", true)).await;
        assert!(!page.contains("data-send-on-save"));
    }

    #[tokio::test]
    async fn a_file_changed_meanwhile_is_not_overwritten() {
        let hub = hub();
        let (_, _, page) = send(&hub, get(&hub, "/instructions/global/AGENTS.md/edit", true)).await;
        let base = base_of(&page);
        fs::write(
            hub.knowledge.join("AGENTS.md"),
            "# Rules\nchanged elsewhere\n",
        )
        .unwrap();
        git(&hub.knowledge, &["commit", "-qam", "elsewhere"]);
        let (status, _, page) = send(
            &hub,
            post(
                &hub,
                "/instructions/global/AGENTS.md/edit",
                &[("csrf", &hub.csrf), ("base", &base), ("text", "# Mine\n")],
                ORIGIN,
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(page.contains("changed since you opened it") && page.contains("# Mine"));
        assert!(
            fs::read_to_string(hub.knowledge.join("AGENTS.md"))
                .unwrap()
                .contains("changed elsewhere")
        );
    }

    #[tokio::test]
    async fn projects_save_only_on_main() {
        let hub = hub();
        git(&hub.project, &["checkout", "-q", "-b", "feature"]);
        let (_, _, page) = send(&hub, get(&hub, "/skills/shop/stock/edit", true)).await;
        let base = base_of(&page);
        let (status, _, page) = send(
            &hub,
            post(
                &hub,
                "/skills/shop/stock/edit",
                &[
                    ("csrf", &hub.csrf),
                    ("base", &base),
                    ("text", "---\ndescription: Count\n---\nNew\n"),
                ],
                ORIGIN,
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert!(page.contains("is on feature, not main"), "{page}");
        assert!(
            fs::read_to_string(hub.project.join(".agents/skills/stock/SKILL.md"))
                .unwrap()
                .ends_with("Body\n")
        );
    }

    #[tokio::test]
    async fn links_are_not_edited_through() {
        let hub = hub();
        fs::write(hub.project.join("real.md"), "# Real\n").unwrap();
        fs::remove_file(hub.project.join("AGENTS.md")).unwrap();
        std::os::unix::fs::symlink("real.md", hub.project.join("AGENTS.md")).unwrap();
        let (status, _, body) =
            send(&hub, get(&hub, "/instructions/shop/AGENTS.md/edit", true)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("link"), "{body}");
    }

    fn json(path: &str, body: &str, origin: &str) -> Request<Body> {
        Request::post(path)
            .header("tailscale-user-login", LOGIN)
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn first_run_trusts_the_tailscale_login_while_open() {
        let hub = hub();
        let (status, _, page) = send(&hub, get(&hub, "/signin", false)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains(LOGIN) && page.contains("Create passkey"),
            "{page}"
        );
        assert!(!page.contains("setup code"));
        let (status, _, body) = send(
            &hub,
            json("/auth/enrol/start", r#"{"name":"Laptop"}"#, ORIGIN),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains(r#""flow""#) && body.contains("publicKey"));
        let (status, ..) = send(
            &hub,
            json("/auth/enrol/start", r#"{"name":"x"}"#, "https://evil.test"),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn a_closed_first_run_points_to_the_setup_command() {
        let hub = hub_with(std::time::Duration::ZERO);
        let (_, _, page) = send(&hub, get(&hub, "/signin", false)).await;
        assert!(
            page.contains("atlas-hub setup") && !page.contains("Create passkey"),
            "{page}"
        );
        let (status, _, body) =
            send(&hub, json("/auth/enrol/start", r#"{"name":"x"}"#, ORIGIN)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("atlas-hub setup"));
    }

    #[tokio::test]
    async fn setup_links_work_once() {
        let hub = hub_with(std::time::Duration::ZERO);
        let token =
            atlas_signer::setup::issue(&hub.state, std::time::Duration::from_mins(15)).unwrap();
        let (status, _, page) = send(&hub, get(&hub, &format!("/setup/{token}"), false)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains(&format!(r#"name="token" value="{token}""#)));
        let start = format!(r#"{{"name":"Phone","token":"{token}"}}"#);
        let (status, _, body) = send(&hub, json("/auth/enrol/start", &start, ORIGIN)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let (status, ..) = send(
            &hub,
            get(&hub, &format!("/setup/{}", "0".repeat(64)), false),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert!(atlas_signer::setup::redeem(&hub.state, &token).unwrap());
        let (status, ..) = send(&hub, get(&hub, &format!("/setup/{token}"), false)).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "used up");
        let (status, ..) = send(&hub, json("/auth/enrol/start", &start, ORIGIN)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
}
