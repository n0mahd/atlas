//! The hub's web UI. Read-only in phase 1.
//!
//! Every request reads a fresh snapshot of the store, so the pages always match git.
//! Access: the hub sits behind `tailscale serve`, which sets `Tailscale-User-Login`;
//! only logins in [`Access`] get in.

mod markdown;
mod roman;
pub mod views;

use std::sync::Arc;

use askama::Template;
use atlas_fs::Sources;
use axum::Router;
use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

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
}

impl AppState {
    #[must_use]
    pub fn new(sources: Sources, hub: &str) -> Self {
        Self {
            sources: Arc::new(sources),
            hub: Arc::from(hub),
        }
    }

    async fn snapshot(&self) -> Result<Snapshot, AppError> {
        let sources = Arc::clone(&self.sources);
        tokio::task::spawn_blocking(move || Snapshot {
            catalog: atlas_fs::load_catalog(&sources),
            harnesses: atlas_fs::scan_harnesses(&sources),
        })
        .await
        .map_err(|_| AppError::Internal)
    }
}

/// Builds the hub's router.
pub fn router(state: AppState, access: Access) -> Router {
    let access = Arc::new(access);
    Router::new()
        .route("/", get(overview))
        .route("/skills", get(skills))
        .route("/skills/{home}/{name}", get(skill_detail))
        .route("/instructions", get(instructions))
        .route("/instructions/{scope}/{file}", get(instruction_detail))
        .route("/memory", get(memory))
        .route("/memory/{name}", get(memory_detail))
        .route("/hooks", get(hooks))
        .route("/mcp", get(mcp))
        .route("/machines", get(machines))
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
        HeaderValue::from_static("no-referrer"),
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
}

#[derive(Template)]
#[template(path = "instructions.html")]
struct InstructionsPage {
    page: Page,
    rows: Vec<views::InstructionRow>,
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
    cards: Vec<views::HarnessCard>,
}

// ---------------------------------------------------------------- handlers

async fn overview(State(s): State<AppState>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/", "Frontispiece", &s.hub, &snap);
    Ok(Html(
        OverviewPage {
            page,
            v: views::overview(&snap),
        }
        .render()?,
    ))
}

#[derive(Debug, Default, Deserialize)]
struct ListQuery {
    #[serde(default)]
    q: String,
    #[serde(default)]
    f: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    host: String,
}

async fn skills(State(s): State<AppState>, Query(q): Query<ListQuery>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/skills", "Skills", &s.hub, &snap);
    Ok(Html(
        SkillsPage {
            page,
            v: views::skills(&snap, &q.q, &q.f),
        }
        .render()?,
    ))
}

async fn skill_detail(
    State(s): State<AppState>,
    Path((home, name)): Path<(String, String)>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::skill_detail(&snap, &home, &name).ok_or(AppError::NotFound)?;
    let page = Page::new("/skills", v.name.clone(), &s.hub, &snap);
    Ok(Html(SkillPage { page, v }.render()?))
}

async fn instructions(State(s): State<AppState>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/instructions", "Instructions", &s.hub, &snap);
    Ok(Html(
        InstructionsPage {
            page,
            rows: views::instructions(&snap),
        }
        .render()?,
    ))
}

async fn instruction_detail(
    State(s): State<AppState>,
    Path((scope, file)): Path<(String, String)>,
) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::instruction_detail(&snap, &scope, &file).ok_or(AppError::NotFound)?;
    let page = Page::new("/instructions", v.title.clone(), &s.hub, &snap);
    Ok(Html(InstructionPage { page, v }.render()?))
}

async fn memory(State(s): State<AppState>, Query(q): Query<ListQuery>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/memory", "Memory", &s.hub, &snap);
    Ok(Html(
        MemoryPage {
            page,
            v: views::memory(&snap, &q.q, &q.kind, &q.host),
        }
        .render()?,
    ))
}

async fn memory_detail(State(s): State<AppState>, Path(name): Path<String>) -> PageResult {
    let snap = s.snapshot().await?;
    let v = views::memory_detail(&snap, &name).ok_or(AppError::NotFound)?;
    let page = Page::new("/memory", v.name.clone(), &s.hub, &snap);
    Ok(Html(MemoryDetailPage { page, v }.render()?))
}

async fn hooks(State(s): State<AppState>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/hooks", "Hooks", &s.hub, &snap);
    Ok(Html(
        HooksPage {
            page,
            v: views::hooks(&snap),
        }
        .render()?,
    ))
}

async fn mcp(State(s): State<AppState>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/mcp", "MCP servers", &s.hub, &snap);
    Ok(Html(McpPage { page }.render()?))
}

async fn machines(State(s): State<AppState>) -> PageResult {
    let snap = s.snapshot().await?;
    let page = Page::new("/machines", "Machines", &s.hub, &snap);
    Ok(Html(
        MachinesPage {
            page,
            cards: views::machines(&snap),
        }
        .render()?,
    ))
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
        "emblem.svg",
        "image/svg+xml",
        include_bytes!("../static/emblem.svg"),
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
