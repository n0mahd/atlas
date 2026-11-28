//! The skill library's pages: New skill, the confirmation page before many switches,
//! moves or deletes, the bulk form, and the read-only file viewer. Every change goes
//! through [`crate::skill_ops`] or [`crate::skills_place`], so through `change`.
//!
//! Without JavaScript each form is a plain POST answered with a redirect; with it the
//! forms carry `data-send-on-save`, so passkey.js sends the change in the same tap.

use askama::Template;
use atlas_core::catalog::SkillHome;
use atlas_core::kernel::{HarnessKind, Scope, Slug};
use atlas_core::untrusted::{Markdown, Untrusted};
use axum::Json;
use axum::extract::{Form, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};

use crate::change::{self, ChangeError, respond};
use crate::edit::{wants_json, writer};
use crate::views::{Page, Snapshot};
use crate::{AppError, AppState, markdown, skill_ops, skills_place};

/// The biggest file the viewer shows.
const MAX_VIEW_BYTES: u64 = 256 * 1024;
/// The most skills one bulk change takes.
const MAX_BULK: usize = 500;

/// A skill home named in a URL or form: `private`, `public` or a registered project.
pub(crate) fn parse_home(s: &AppState, key: &str) -> Option<SkillHome> {
    match key {
        "private" => Some(SkillHome::Private),
        "public" => s.sources.public_skills.as_ref().map(|_| SkillHome::Public),
        p => s
            .sources
            .projects
            .iter()
            .find(|x| x.name.as_str() == p)
            .map(|x| SkillHome::Project(x.name.clone())),
    }
}

/// `home/name` from a checkbox or hidden field.
fn parse_skill(s: &AppState, key: &str) -> Option<(SkillHome, Slug)> {
    let (home, name) = key.split_once('/')?;
    Some((parse_home(s, home)?, Slug::new(name).ok()?))
}

/// A path back into the Skills pages, never anywhere else.
fn safe_back(back: &str) -> String {
    let ok = back == "/skills"
        || back.starts_with("/skills?")
        || back.starts_with("/skills/") && !back.starts_with("//");
    let clean = !back.contains(['\\', '\r', '\n']) && !back.contains("://");
    if ok && clean {
        back.to_owned()
    } else {
        "/skills".to_owned()
    }
}

use crate::library::pct;

fn pct_path(s: &str) -> String {
    s.split('/').map(pct).collect::<Vec<_>>().join("/")
}

// ---------------------------------------------------------------- bulk

/// The bulk form's fields. Repeated `s` fields name the skills, so it is parsed by hand
/// (serde's form decoding keeps only one value per name).
#[derive(Debug, Default)]
struct Bulk {
    csrf: String,
    action: String,
    skills: Vec<String>,
    agent: String,
    to: String,
    back: String,
    confirmed: bool,
}

impl Bulk {
    fn parse(text: &str) -> Self {
        let mut b = Self::default();
        for (k, v) in form_urlencoded::parse(text.as_bytes()) {
            match k.as_ref() {
                "csrf" => b.csrf = v.into_owned(),
                "action" => b.action = v.into_owned(),
                "s" if b.skills.len() <= MAX_BULK => b.skills.push(v.into_owned()),
                "agent" => b.agent = v.into_owned(),
                "to" => b.to = v.into_owned(),
                "back" => b.back = v.into_owned(),
                "confirmed" => b.confirmed = v == "1",
                _ => {}
            }
        }
        b
    }

    /// The confirmation page's address for this request (no CSRF token in it).
    fn confirm_url(&self) -> String {
        let mut q = form_urlencoded::Serializer::new(String::new());
        q.append_pair("action", &self.action);
        for s in &self.skills {
            q.append_pair("s", s);
        }
        if !self.agent.is_empty() {
            q.append_pair("agent", &self.agent);
        }
        if !self.to.is_empty() {
            q.append_pair("to", &self.to);
        }
        q.append_pair("back", &safe_back(&self.back));
        format!("/skills/confirm?{}", q.finish())
    }
}

/// What a bulk request asks for, checked.
struct Asked {
    action: Action,
    skills: Vec<(SkillHome, Slug)>,
    agent: Option<HarnessKind>,
    to: Option<SkillHome>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Action {
    On,
    Off,
    Move,
    Delete,
}

fn check(s: &AppState, b: &Bulk) -> Result<Asked, String> {
    let action = match b.action.as_str() {
        "on" => Action::On,
        "off" => Action::Off,
        "move" => Action::Move,
        "delete" => Action::Delete,
        _ => return Err("Pick what to do with the chosen skills.".to_owned()),
    };
    if b.skills.is_empty() {
        return Err("Pick at least one skill first.".to_owned());
    }
    if b.skills.len() > MAX_BULK {
        return Err(format!("Atlas changes at most {MAX_BULK} skills at once."));
    }
    let mut skills = Vec::with_capacity(b.skills.len());
    for key in &b.skills {
        let skill = parse_skill(s, key).ok_or_else(|| format!("{key} isn't a skill in Atlas."))?;
        if !skills.contains(&skill) {
            skills.push(skill);
        }
    }
    let agent = if b.agent.is_empty() {
        None
    } else {
        let k: HarnessKind = b
            .agent
            .parse()
            .map_err(|_| format!("{} isn't an agent Atlas knows.", b.agent))?;
        if k.is_host() {
            return Err(format!("{} has no skills of its own.", k.display_name()));
        }
        Some(k)
    };
    let to = if action == Action::Move {
        Some(parse_home(s, &b.to).ok_or("Pick where to move the skills.")?)
    } else {
        None
    };
    Ok(Asked {
        action,
        skills,
        agent,
        to,
    })
}

#[derive(Serialize)]
struct Next {
    next: String,
    send: bool,
}

/// POST /skills/bulk: turn many on or off, move or delete them. Turning several off,
/// moving and deleting first go to the confirmation page; its form comes back here with
/// `confirmed=1`.
pub(crate) async fn bulk(State(s): State<AppState>, headers: HeaderMap, body: String) -> Response {
    let b = Bulk::parse(&body);
    if let Err(e) = change::authorise(&s, &headers, &b.csrf) {
        return e.into_response(&headers);
    }
    let asked = match check(&s, &b) {
        Ok(a) => a,
        Err(msg) => return change::form_error(&headers, AppError::Bad(msg)),
    };
    let needs_confirm = match asked.action {
        Action::On => false,
        Action::Off => asked.skills.len() > 1,
        Action::Move | Action::Delete => true,
    };
    if needs_confirm && !b.confirmed {
        let next = b.confirm_url();
        return if wants_json(&headers) {
            Json(Next { next, send: false }).into_response()
        } else {
            Redirect::to(&next).into_response()
        };
    }
    let back = safe_back(&b.back);
    let result = match asked.action {
        Action::On | Action::Off => {
            skills_place::set_agents(
                &s,
                &headers,
                &b.csrf,
                asked.skills,
                asked.agent,
                asked.action == Action::On,
            )
            .await
        }
        Action::Move => {
            let to = asked.to.unwrap_or(SkillHome::Private);
            skill_ops::move_skills(&s, &headers, &b.csrf, asked.skills, to).await
        }
        Action::Delete => skill_ops::delete_skills(&s, &headers, &b.csrf, asked.skills).await,
    };
    match result {
        Ok(outcome) => respond(&s, &headers, &outcome, &back),
        Err(ChangeError::Refused(msg)) if !wants_json(&headers) => {
            refused_page(&s, &headers, &msg, &back).await
        }
        Err(e) => e.into_response(&headers),
    }
}

// ---------------------------------------------------------------- confirmation

#[derive(Template)]
#[template(path = "confirm.html")]
struct ConfirmPage {
    page: Page,
    lines: Vec<String>,
    notes: Vec<String>,
    error: String,
    button: String,
    action: String,
    skills: Vec<String>,
    agent: String,
    to: String,
    back: String,
    danger: bool,
}

async fn refused_page(s: &AppState, headers: &HeaderMap, msg: &str, back: &str) -> Response {
    let Ok(snap) = s.snapshot().await else {
        return AppError::Internal.into_response();
    };
    let page = s.page(
        headers,
        "/skills",
        "Nothing changed",
        &snap,
        &Scope::Global,
        false,
    );
    let html = ConfirmPage {
        page,
        lines: Vec::new(),
        notes: Vec::new(),
        error: msg.to_owned(),
        button: String::new(),
        action: String::new(),
        skills: Vec::new(),
        agent: String::new(),
        to: String::new(),
        back: back.to_owned(),
        danger: false,
    }
    .render();
    match html {
        Ok(h) => (StatusCode::CONFLICT, Html(h)).into_response(),
        Err(_) => AppError::Internal.into_response(),
    }
}

fn plural(n: usize, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

/// GET /skills/confirm: what a change will do, before it does it.
pub(crate) async fn confirm(
    State(s): State<AppState>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Result<Response, AppError> {
    let owner = writer(&s)?;
    let query = query.unwrap_or_default();
    if owner.auth.session(&headers).is_none() {
        let next = format!("/skills/confirm?{query}");
        return Ok(Redirect::to(&format!("/signin?next={}", pct(&next))).into_response());
    }
    let bulk = Bulk::parse(&query);
    let back = safe_back(&bulk.back);
    let asked = check(&s, &bulk).map_err(AppError::Bad)?;
    let count = asked.skills.len();
    let names = || {
        asked
            .skills
            .iter()
            .map(|(home, name)| format!("{}/{name}", home.key()))
            .collect::<Vec<_>>()
    };
    let agent_words = asked
        .agent
        .map_or_else(|| "every agent".to_owned(), |k| k.display_name().to_owned());
    let (question, button, preview, danger) = match asked.action {
        Action::On | Action::Off => {
            let verb = if asked.action == Action::On {
                "on"
            } else {
                "off"
            };
            (
                format!("Turn {} {verb} for {agent_words}?", plural(count, "skill")),
                format!("Turn {verb}"),
                Ok(skill_ops::Preview {
                    lines: names(),
                    notes: Vec::new(),
                }),
                false,
            )
        }
        Action::Move => {
            let to = asked.to.clone().unwrap_or(SkillHome::Private);
            let words = match &to {
                SkillHome::Project(p) => p.to_string(),
                _ => "Global".to_owned(),
            };
            (
                format!("Move {} to {words}?", plural(count, "skill")),
                "Move".to_owned(),
                skill_ops::preview_move(&s, asked.skills.clone(), to).await,
                false,
            )
        }
        Action::Delete => (
            format!("Delete {}?", plural(count, "skill")),
            format!("Delete {}", plural(count, "skill")),
            skill_ops::preview_delete(&s, asked.skills.clone()).await,
            true,
        ),
    };
    let snap = s.snapshot().await?;
    let page = s.page(
        &headers,
        "/skills",
        question.clone(),
        &snap,
        &Scope::Global,
        false,
    );
    let (lines, notes, error) = match preview {
        Ok(p) => (p.lines, p.notes, String::new()),
        Err(e) => (Vec::new(), Vec::new(), e),
    };
    Ok(Html(
        ConfirmPage {
            page,
            lines,
            notes,
            error,
            button,
            action: bulk.action.clone(),
            skills: asked
                .skills
                .iter()
                .map(|(home, name)| format!("{}/{name}", home.key()))
                .collect(),
            agent: bulk.agent.clone(),
            to: bulk.to.clone(),
            back,
            danger,
        }
        .render()?,
    )
    .into_response())
}

// ---------------------------------------------------------------- new skill

#[derive(Template)]
#[template(path = "skill_new.html")]
struct NewPage {
    page: Page,
    name: String,
    description: String,
    /// `global` or the project it would go in.
    place: String,
    project: Option<String>,
    error: String,
    max: usize,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct NewQuery {
    #[serde(default)]
    scope: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct NewForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default, rename = "where")]
    place: String,
}

fn new_page(
    s: &AppState,
    headers: &HeaderMap,
    snap: &Snapshot,
    form: &NewForm,
    project: Option<String>,
    error: String,
) -> Result<Response, AppError> {
    let scope = project
        .as_deref()
        .map_or(Scope::Global, |p| crate::views::resolve_scope(snap, p));
    let page = s.page(headers, "/skills", "New skill", snap, &scope, false);
    let status = if error.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::CONFLICT
    };
    let place = if form.place.is_empty() {
        project.clone().unwrap_or_else(|| "global".to_owned())
    } else {
        form.place.clone()
    };
    Ok((
        status,
        Html(
            NewPage {
                page,
                name: form.name.clone(),
                description: form.description.clone(),
                place,
                project,
                error,
                max: skill_ops::MAX_DESCRIPTION,
            }
            .render()?,
        ),
    )
        .into_response())
}

/// GET /skills/new: name, one-line description, and this project or Global.
pub(crate) async fn new_form(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<NewQuery>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if w.auth.session(&headers).is_none() {
        return Ok(Redirect::to("/signin?next=/skills/new").into_response());
    }
    let snap = s.snapshot().await?;
    let project = crate::views::resolve_scope(&snap, &q.scope)
        .project()
        .map(ToString::to_string);
    new_page(
        &s,
        &headers,
        &snap,
        &NewForm::default(),
        project,
        String::new(),
    )
}

/// POST /skills/new: writes `SKILL.md`, on for every agent, then opens the skill.
pub(crate) async fn new_save(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<NewForm>,
) -> Response {
    if let Err(e) = change::authorise(&s, &headers, &f.csrf) {
        return e.into_response(&headers);
    }
    new_skill(&s, &headers, &f)
        .await
        .unwrap_or_else(|e| change::form_error(&headers, e))
}

async fn new_skill(s: &AppState, headers: &HeaderMap, f: &NewForm) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let home = if f.place == "global" || f.place.is_empty() {
        Some(SkillHome::Private)
    } else {
        parse_home(s, &f.place).filter(|h| matches!(h, SkillHome::Project(_)))
    };
    let project = match &home {
        Some(SkillHome::Project(p)) => Some(p.to_string()),
        _ => None,
    };
    let json = wants_json(headers);
    let fail = |msg: String| async {
        if json {
            Ok(ChangeError::Refused(msg).into_response(headers))
        } else {
            new_page(s, headers, &snap, f, project.clone(), msg)
        }
    };
    let Some(home) = home else {
        return fail("Pick Global or a registered project.".to_owned()).await;
    };
    let Ok(name) = Slug::new(f.name.trim()) else {
        return fail(
            "Name it with lowercase letters, digits and single hyphens, such as pdf-tools (at most 64)."
                .to_owned(),
        )
        .await;
    };
    let next = format!("/skills/{}/{name}", home.key());
    match skill_ops::create_skill(s, headers, &f.csrf, home, name, f.description.clone()).await {
        Ok(outcome) => Ok(respond(s, headers, &outcome, &next)),
        Err(ChangeError::Refused(msg)) => fail(msg).await,
        Err(e) => Ok(e.into_response(headers)),
    }
}

// ---------------------------------------------------------------- files

#[derive(Debug)]
pub struct FileEntry {
    pub rel: String,
    /// The last part of the path, and the folders above it (`scripts/`).
    pub leaf: String,
    pub dir: String,
    pub href: String,
    pub script: bool,
}

/// A skill folder's files for its page, its scripts, and the menu.
#[derive(Debug, Default)]
pub struct FileTree {
    pub folder: String,
    pub entries: Vec<FileEntry>,
    pub scripts: Vec<String>,
    /// The Delete confirmation, when the skill can be deleted here.
    pub delete_href: Option<String>,
    /// The catalog stopped listing at its limit.
    pub truncated: bool,
}

const SCRIPT_EXTENSIONS: [&str; 16] = [
    "sh", "bash", "zsh", "fish", "py", "js", "mjs", "cjs", "ts", "rb", "pl", "php", "ps1", "bat",
    "cmd", "exe",
];

fn is_script(rel: &str) -> bool {
    let ext = std::path::Path::new(rel)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    ext.is_some_and(|e| SCRIPT_EXTENSIONS.contains(&e.as_str()))
        || rel.starts_with("scripts/")
        || rel.starts_with("bin/")
}

/// The file tree for a skill's page.
#[must_use]
pub(crate) fn file_tree(snap: &Snapshot, home: &str, name: &str, can_delete: bool) -> FileTree {
    let Some(skill) = snap
        .catalog
        .skills
        .iter()
        .find(|s| s.home.key() == home && s.name.as_str() == name)
    else {
        return FileTree::default();
    };
    let base = format!("/skills/{}/{}/files/", pct(home), pct(name));
    let mut files: Vec<&str> = vec!["SKILL.md"];
    files.extend(skill.files.iter().map(String::as_str));
    let entries = files
        .iter()
        .map(|rel| FileEntry {
            rel: (*rel).to_owned(),
            leaf: rel.rsplit('/').next().unwrap_or(rel).to_owned(),
            dir: rel
                .rsplit_once('/')
                .map_or_else(String::new, |(d, _)| format!("{d}/")),
            href: format!("{base}{}", pct_path(rel)),
            script: is_script(rel),
        })
        .collect::<Vec<_>>();
    let scripts = entries
        .iter()
        .filter(|e| e.script)
        .map(|e| e.rel.clone())
        .collect();
    FileTree {
        folder: skill.path.trim_end_matches("/SKILL.md").to_owned(),
        entries,
        scripts,
        delete_href: (can_delete && !matches!(skill.home, SkillHome::Public)).then(|| {
            let back = match &skill.home {
                SkillHome::Project(p) => format!("/skills?scope={p}"),
                _ => "/skills".to_owned(),
            };
            format!(
                "/skills/confirm?action=delete&s={}&back={}",
                pct(&format!("{home}/{name}")),
                pct(&back)
            )
        }),
        truncated: skill.files.len() >= 199,
    }
}

#[derive(Template)]
#[template(path = "skill_file.html")]
struct FilePage {
    page: Page,
    skill_href: String,
    skill: String,
    shown: String,
    html: String,
    text: String,
    notice: &'static str,
    script: bool,
}

/// GET /skills/{home}/{name}/files/{*path}: one file of a skill, read-only. Only a file
/// the catalog listed for that skill is opened, never a path taken from the request.
pub(crate) async fn file_view(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name, rel)): Path<(String, String, String)>,
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let skill = snap
        .catalog
        .skills
        .iter()
        .find(|x| x.home.key() == home && x.name.as_str() == name)
        .ok_or(AppError::NotFound)?;
    let listed = if rel == "SKILL.md" {
        Some("SKILL.md")
    } else {
        skill.files.iter().map(String::as_str).find(|f| *f == rel)
    };
    let file = listed.ok_or(AppError::NotFound)?.to_owned();
    let (root, folder) =
        atlas_fs::skill_folder::locate(&s.sources, &skill.home, skill.name.as_str())
            .ok_or(AppError::NotFound)?;
    let shown = s.sources.display(&root.join(&folder).join(&file));
    let read_rel = file.clone();
    let read = tokio::task::spawn_blocking(move || {
        atlas_fs::skill_folder::read_file(&root, &folder, &read_rel, MAX_VIEW_BYTES)
    })
    .await
    .map_err(|_| AppError::Internal)?;
    let (mut html, mut text, mut notice) = (String::new(), String::new(), "");
    match read {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(t) if !t.contains('\0') => {
                if std::path::Path::new(&file)
                    .extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case("md"))
                {
                    let names = snap
                        .catalog
                        .memory
                        .iter()
                        .map(|m| m.name.as_str())
                        .collect();
                    html = markdown::render(&Untrusted::new(Markdown(t)), &names);
                } else {
                    text = t;
                }
            }
            _ => notice = "This file isn't text, so Atlas doesn't show it.",
        },
        Err(atlas_fs::skill_folder::ViewError::TooBig) => {
            notice = "This file is larger than 256 KB; open it in an editor to read it.";
        }
        Err(atlas_fs::skill_folder::ViewError::Link) => {
            return Err(AppError::Bad(
                "That file is a link; Atlas doesn't follow links in the store.".to_owned(),
            ));
        }
        Err(atlas_fs::skill_folder::ViewError::Missing) => return Err(AppError::NotFound),
    }
    let scope = crate::views::resolve_scope(&snap, &home);
    let page = s.page(&headers, "/skills", file.clone(), &snap, &scope, false);
    Ok(Html(
        FilePage {
            page,
            skill_href: format!("/skills/{}/{}", pct(&home), pct(&name)),
            skill: name.clone(),
            script: is_script(&file),
            shown,
            html,
            text,
            notice,
        }
        .render()?,
    )
    .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn back_links_stay_on_the_skills_pages() {
        assert_eq!(safe_back("/skills?scope=shop"), "/skills?scope=shop");
        assert_eq!(safe_back("//evil.example/skills"), "/skills");
        assert_eq!(safe_back("https://evil.example"), "/skills");
        assert_eq!(safe_back("/settings"), "/skills");
    }

    #[test]
    fn bulk_forms_keep_every_skill() {
        let b = Bulk::parse("csrf=t&action=delete&s=private%2Fa&s=shop%2Fb&confirmed=1");
        assert_eq!(b.skills, ["private/a", "shop/b"]);
        assert!(b.confirmed);
        assert!(!b.confirm_url().contains("csrf"));
    }

    #[test]
    fn scripts_are_spotted() {
        assert!(is_script("scripts/anything"));
        assert!(is_script("tool.PY"));
        assert!(!is_script("references/notes.md"));
    }
}
