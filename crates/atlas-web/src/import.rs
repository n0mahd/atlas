//! Bringing things into Atlas (decision 0027, `documents/skills.md` "Bringing things in"):
//! the Outside Atlas parts of the Skills, Memory and Hooks pages, their row actions and
//! Bring all in, Leave it outside, and the page that compares two copies of a skill.
//!
//! Every action names its item by the key the page showed (`skill:~/.claude/skills/x`,
//! `memory:<folder>/<file>`, `hook:<config>#<fingerprint>`) and re-finds it in a fresh scan
//! of this machine, so no path from a request is ever opened. Each is one change through
//! [`crate::change::commit_change`]: a passkey session, the git lock, clean paths, one
//! commit per repo. Atlas copies; atlasd later swaps the original for Atlas's link on each
//! machine, and only where it is an exact copy.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use askama::Template;
use atlas_core::catalog::SkillHome;
use atlas_core::catalog::edit::RepoKind;
use atlas_core::kernel::{Scope, Slug};
use atlas_fs::library::{self, Tree};
use atlas_fs::outside::{self, NoteState, Origin, OutsideHook, OutsideSkill};
use axum::Form;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::change::{self, Apply, ChangeError, Outcome, RepoChange};
use crate::views::{self, AgentMark, Page, Snapshot};
use crate::{AppError, AppState, icons};

const GONE: &str =
    "That isn't outside Atlas any more, or it changed. Reload the page; nothing changed.";

/// The sentence a `done=` code adds to a page's notice after an import.
#[must_use]
pub(crate) fn done_text(code: &str) -> Option<String> {
    if let Some(n) = code.strip_prefix("skipped-")
        && let Ok(n) = n.parse::<usize>()
    {
        let what = if n == 1 {
            "1 entry inside the folder (a link, .git or a special file) wasn't".to_owned()
        } else {
            format!("{n} entries inside the folder (links, .git or special files) weren't")
        };
        return Some(format!(
            "{what} copied, so atlasd leaves the original where it is; delete it by hand once Atlas's copy works."
        ));
    }
    Some(
        match code {
            "already" => "Atlas has the same skill already. atlasd puts Atlas's link in its place with the next update.",
            "kept" => "It stays outside Atlas and isn't offered again.",
            "hook" => "After the next update, atlasd's own entry takes the hand-made one's place.",
            "notes" => "Once every note there is in Atlas, atlasd links Claude Code's folder to Atlas's.",
            "theirs" => "Atlas now holds this machine's copy.",
            _ => return None,
        }
        .to_owned(),
    )
}

// ---------------------------------------------------------------- view models

/// One outside skill on the Skills page.
#[derive(Debug, Clone)]
pub struct SkillRow {
    pub key: String,
    pub name: String,
    pub source: String,
    pub flag: String,
    pub marks: Vec<AgentMark>,
    pub all: bool,
    /// `import`, `same` (Atlas has it already), `differs` (Atlas has another copy) or
    /// `blocked`.
    pub state: &'static str,
    pub entries: String,
    /// The project it was found in, which it may go to instead of Global.
    pub project: Option<String>,
    pub compare: Option<String>,
    /// What a copy leaves out, as one line; atlasd won't replace such an original.
    pub left_out: String,
}

#[derive(Debug, Clone)]
pub struct BuiltInRow {
    pub icon: &'static str,
    pub agent: &'static str,
    pub name: String,
    pub source: String,
    pub path: String,
}

/// The Outside Atlas and Built in parts of the Skills page.
#[derive(Debug, Default)]
pub struct SkillsOutside {
    /// Shown only on the Global scope: these are the login's own folders.
    pub show: bool,
    pub rows: Vec<SkillRow>,
    pub built_in: Vec<BuiltInRow>,
    /// Rows Bring all in would copy.
    pub importable: usize,
    pub kept: usize,
    /// Things Tidy up can do; the link shows when there are any.
    pub tidy: usize,
}

fn skill_source(item: &OutsideSkill) -> String {
    match &item.origin {
        Origin::OwnFolder => format!("Its own folder in {}", parent_of(&item.entries[0])),
        Origin::Linked { target } | Origin::Tool { target, .. } => format!("Linked from {target}"),
        Origin::Broken { target } => format!("Broken link to {target}"),
    }
}

fn parent_of(shown: &str) -> &str {
    shown.rsplit_once('/').map_or(shown, |(p, _)| p)
}

fn skill_row(snap: &Snapshot, item: &OutsideSkill) -> SkillRow {
    let (marks, all) = views::agent_marks(snap, &item.kinds);
    let (state, flag) = match (&item.clash, item.blocked()) {
        (_, Some(reason)) => ("blocked", reason),
        (Some(c), None) if c.same => ("same", "Already in Atlas".to_owned()),
        (Some(_), None) => ("differs", "Atlas has another copy".to_owned()),
        (None, None) => ("import", String::new()),
    };
    SkillRow {
        key: item.key.clone(),
        name: item.name.clone(),
        source: skill_source(item),
        flag,
        marks,
        all,
        state,
        entries: item.entries.join(", "),
        project: item.project.as_ref().map(ToString::to_string),
        compare: (state == "differs").then(|| format!("/import/compare?key={}", query(&item.key))),
        left_out: item.left_out.join(", "),
    }
}

/// Percent-encodes a query value.
pub(crate) fn query(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Whether a skill row is for Bring all in: importable, not left outside, no clash.
fn plain_import(item: &OutsideSkill) -> bool {
    !item.kept && item.blocked().is_none() && item.clash.is_none()
}

#[must_use]
pub(crate) fn skills_outside(snap: &Snapshot, scope: &Scope, q: &str) -> SkillsOutside {
    let o = &snap.outside;
    let q = q.trim().to_lowercase();
    let wanted = |name: &str| q.is_empty() || name.to_lowercase().contains(&q);
    SkillsOutside {
        show: scope == &Scope::Global,
        rows: o
            .skills
            .iter()
            .filter(|s| !s.kept && wanted(&s.name))
            .map(|s| skill_row(snap, s))
            .collect(),
        built_in: o
            .built_in
            .iter()
            .filter(|b| wanted(&b.name))
            .map(|b| BuiltInRow {
                icon: icons::icon(b.agent),
                agent: views::agent_name(b.agent),
                name: b.name.clone(),
                source: b.source.clone(),
                path: b.path.clone(),
            })
            .collect(),
        importable: o.skills.iter().filter(|s| plain_import(s)).count(),
        kept: o.skills.iter().filter(|s| s.kept).count(),
        tidy: crate::tidy::count(snap),
    }
}

/// One of Claude Code's notes on the Memory page.
#[derive(Debug, Clone)]
pub struct NoteRow {
    pub key: String,
    pub file: String,
    /// `new`, `same` or `differs`.
    pub state: &'static str,
    pub label: &'static str,
}

#[derive(Debug, Clone)]
pub struct NotesGroup {
    pub project: String,
    pub folder: String,
    pub rows: Vec<NoteRow>,
    pub new: usize,
    pub problem: Option<String>,
}

/// The Outside Atlas part of the Memory page.
#[derive(Debug, Default)]
pub struct MemoryOutside {
    pub groups: Vec<NotesGroup>,
    /// Folders for repos Atlas doesn't know, by raw name, and how many notes each has.
    pub stray: Vec<(String, usize)>,
    pub new: usize,
    /// The project a project-scoped page shows, for Bring all in.
    pub project: String,
}

#[must_use]
pub(crate) fn memory_outside(snap: &Snapshot, scope: &Scope) -> MemoryOutside {
    let groups: Vec<NotesGroup> = snap
        .outside
        .memory
        .iter()
        .filter(|p| scope.project().is_none_or(|s| s == &p.project))
        .map(|p| {
            let rows: Vec<NoteRow> = p
                .notes
                .iter()
                .filter(|n| !n.kept)
                .map(|n| {
                    let (state, label) = match n.state {
                        NoteState::New => ("new", "Not in Atlas"),
                        NoteState::Same => ("same", "In Atlas"),
                        NoteState::Differs => ("differs", "Atlas's copy differs"),
                    };
                    NoteRow {
                        key: n.key.clone(),
                        file: n.file.clone(),
                        state,
                        label,
                    }
                })
                .collect();
            NotesGroup {
                project: p.project.to_string(),
                folder: p.shown.clone(),
                new: rows.iter().filter(|r| r.state == "new").count(),
                rows,
                problem: p.problem.clone(),
            }
        })
        .collect();
    MemoryOutside {
        new: groups.iter().map(|g| g.new).sum(),
        groups,
        stray: if scope == &Scope::Global {
            snap.outside
                .stray_memory
                .iter()
                .map(|s| (s.key.clone(), s.notes))
                .collect()
        } else {
            Vec::new()
        },
        project: scope
            .project()
            .map_or_else(String::new, ToString::to_string),
    }
}

/// One hook found in a harness's config, on the Hooks page.
#[derive(Debug, Clone)]
pub struct HookRow {
    pub key: String,
    pub icon: &'static str,
    pub harness: &'static str,
    /// Secrets masked.
    pub command: String,
    pub event: String,
    pub config: String,
    pub timeout: String,
    pub flag: String,
    pub name: String,
    pub importable: bool,
}

#[derive(Debug, Default)]
pub struct HooksOutside {
    pub rows: Vec<HookRow>,
    pub importable: usize,
}

/// Why a hook can't come in: [`OutsideHook::blocked`], or a secret in its command.
fn hook_blocked(h: &OutsideHook) -> Option<String> {
    h.blocked().or_else(|| {
        (views::mask_secrets(&h.command) != h.command).then(|| {
            "Holds what looks like a secret; secrets never go in git. Move it to an environment variable first".to_owned()
        })
    })
}

#[must_use]
pub(crate) fn hooks_outside(snap: &Snapshot) -> HooksOutside {
    let rows: Vec<HookRow> = snap
        .outside
        .hooks
        .iter()
        .filter(|h| !h.kept)
        .map(|h| {
            let flag = hook_blocked(h);
            HookRow {
                key: h.key.clone(),
                icon: icons::icon(h.kind),
                harness: views::agent_name(h.kind),
                command: views::mask_secrets(&h.command),
                event: format!(
                    "{} · {}",
                    h.event,
                    h.matcher.as_deref().unwrap_or("every tool")
                ),
                config: h.config.clone(),
                timeout: h.timeout.map_or_else(String::new, |t| format!("{t}s")),
                importable: flag.is_none(),
                flag: flag.unwrap_or_default(),
                name: h.name.clone(),
            }
        })
        .collect();
    HooksOutside {
        importable: rows.iter().filter(|r| r.importable).count(),
        rows,
    }
}

/// The Home page's notice: how many outside skills could come in, linking to Tidy up.
#[must_use]
pub(crate) fn overview_count(snap: &Snapshot) -> usize {
    snap.outside
        .skills
        .iter()
        .filter(|s| !s.kept && s.origin.importable())
        .count()
}

// ---------------------------------------------------------------- writing

/// One thing a change writes inside a repo.
pub(crate) enum Write {
    /// A new folder from a tree; `replace` removes what is there first.
    Tree {
        rel: String,
        tree: Tree,
        replace: bool,
    },
    File {
        rel: String,
        bytes: Vec<u8>,
    },
    Remove {
        rel: String,
    },
    /// An edit of `atlas/placements.toml`, read and written under the change's lock.
    Placements(PlacementsEdit),
}

pub(crate) type PlacementsEdit = Box<dyn Fn(&mut atlas_core::catalog::Placements) + Send>;

impl Write {
    fn rel(&self) -> &str {
        match self {
            Self::Tree { rel, .. } | Self::File { rel, .. } | Self::Remove { rel } => rel,
            Self::Placements(_) => atlas_core::catalog::PLACEMENTS_FILE,
        }
    }

    fn run(&self, root: &Path) -> Result<(), String> {
        let fail = |e: library::LibraryError| format!("{e}.");
        match self {
            Self::Tree { rel, tree, replace } => {
                if *replace {
                    library::remove_tree(root, rel).map_err(fail)?;
                }
                library::write_tree(root, rel, tree).map_err(fail)
            }
            Self::File { rel, bytes } => library::write_file(root, rel, bytes).map_err(fail),
            Self::Remove { rel } => library::remove_tree(root, rel).map_err(fail),
            Self::Placements(edit) => {
                let (mut placements, expected) = library::read_placements(root).map_err(fail)?;
                edit(&mut placements);
                library::write_placements(root, &placements, &expected)
                    .map(|_| ())
                    .map_err(fail)
            }
        }
    }
}

/// One repo's writes and its commit message.
pub(crate) struct Plan {
    pub kind: RepoKind,
    pub root: PathBuf,
    pub writes: Vec<Write>,
    pub message: String,
}

/// The repo changes and the writer for a list of plans, in order.
pub(crate) fn to_change(plans: Vec<Plan>) -> (Vec<RepoChange>, Apply) {
    let repos = plans
        .iter()
        .map(|p| {
            let mut paths: Vec<PathBuf> = Vec::new();
            for w in &p.writes {
                let rel = PathBuf::from(w.rel());
                if !paths.contains(&rel) {
                    paths.push(rel);
                }
            }
            RepoChange {
                kind: p.kind.clone(),
                root: p.root.clone(),
                paths,
                message: p.message.clone(),
            }
        })
        .collect();
    let apply: Apply = Box::new(move || {
        for p in &plans {
            for w in &p.writes {
                w.run(&p.root)?;
            }
        }
        Ok(())
    });
    (repos, apply)
}

fn knowledge(s: &AppState, writes: Vec<Write>, message: String) -> Plan {
    Plan {
        kind: RepoKind::Knowledge,
        root: s.sources.knowledge.clone(),
        writes,
        message,
    }
}

/// Reads an outside skill's real folder, which must be in the login's home.
pub(crate) fn read_source(s: &AppState, item: &OutsideSkill) -> Result<Tree, String> {
    let real = item.real.as_ref().ok_or_else(|| GONE.to_owned())?;
    let home = std::fs::canonicalize(&s.sources.home).unwrap_or_else(|_| s.sources.home.clone());
    if !real.starts_with(&home) {
        return Err("Atlas copies skills only from your home folder.".to_owned());
    }
    library::read_tree(real).map_err(|e| format!("{} can't be copied: {e}.", item.shown_real))
}

/// Where an imported skill goes: Global (private), or the project it was found in.
pub(crate) fn destination(
    s: &AppState,
    item: &OutsideSkill,
    to: &str,
) -> Result<(RepoKind, PathBuf, String), String> {
    if to.is_empty() || to == "private" {
        return Ok((
            RepoKind::Knowledge,
            s.sources.knowledge.clone(),
            format!("skills/{}", item.name),
        ));
    }
    let project = item
        .project
        .as_ref()
        .filter(|p| p.as_str() == to)
        .ok_or_else(|| "A skill goes to Global or to the project it was found in.".to_owned())?;
    let root = s
        .sources
        .projects
        .iter()
        .find(|p| &p.name == project)
        .map(|p| p.root.clone())
        .ok_or_else(|| GONE.to_owned())?;
    Ok((
        RepoKind::Project(project.clone()),
        root,
        format!(".agents/skills/{}", item.name),
    ))
}

// ---------------------------------------------------------------- handlers

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ItemForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    key: String,
    /// For a skill: `private` or the project it was found in.
    #[serde(default)]
    to: String,
    /// For a hook: its folder name under `hooks/`.
    #[serde(default)]
    name: String,
    /// For memory: the project-scoped page to go back to.
    #[serde(default)]
    scope: String,
    /// For a note whose name Atlas has: use this machine's copy.
    #[serde(default)]
    replace: Option<String>,
}

/// Session and CSRF first, so a stranger learns nothing about what's outside.
pub(crate) fn rejected(s: &AppState, headers: &HeaderMap, csrf: &str) -> Option<Response> {
    let ok = crate::edit::writer(s).is_ok_and(|w| w.authorised(headers, csrf));
    (!ok).then(|| ChangeError::Forbidden.into_response(headers))
}

pub(crate) fn refuse(headers: &HeaderMap, msg: impl Into<String>) -> Response {
    ChangeError::Refused(msg.into()).into_response(headers)
}

/// A fresh read of the store and this machine, for re-finding what a request names.
pub(crate) async fn fresh(s: &AppState) -> Result<Arc<Snapshot>, AppError> {
    s.invalidate().await;
    s.snapshot().await
}

async fn commit(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    plans: Vec<Plan>,
    next: &str,
) -> Response {
    let (repos, apply) = to_change(plans);
    match change::commit_change(s, headers, csrf, repos, apply).await {
        Ok(outcome) => change::respond(s, headers, &outcome, next),
        Err(e) => e.into_response(headers),
    }
}

fn with_done(path: &str, done: &str) -> String {
    let sep = if path.contains('?') { '&' } else { '?' };
    format!("{path}{sep}done={done}")
}

/// Brings one outside skill into Atlas.
pub(crate) async fn bring_skill(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let Some(item) = snap.outside.skills.iter().find(|x| x.key == f.key) else {
        return refuse(&headers, GONE);
    };
    if let Some(reason) = item.blocked() {
        return refuse(&headers, format!("{reason}; Atlas doesn't bring it in."));
    }
    match &item.clash {
        Some(c) if c.same => {
            return change::respond(
                &s,
                &headers,
                &Outcome::Unchanged,
                &with_done("/skills", "already"),
            );
        }
        Some(_) => {
            return axum::response::Redirect::to(&format!(
                "/import/compare?key={}",
                query(&item.key)
            ))
            .into_response();
        }
        None => {}
    }
    let (kind, root, rel) = match destination(&s, item, &f.to) {
        Ok(d) => d,
        Err(m) => return refuse(&headers, m),
    };
    let tree = match read_source(&s, item) {
        Ok(t) => t,
        Err(m) => return refuse(&headers, m),
    };
    let skipped = tree.skipped.len();
    let plan = Plan {
        kind,
        root,
        message: format!("Bring {} into Atlas from {}", item.name, item.entries[0]),
        writes: vec![Write::Tree {
            rel,
            tree,
            replace: false,
        }],
    };
    let next = if skipped > 0 {
        with_done("/skills", &format!("skipped-{skipped}"))
    } else {
        "/skills".to_owned()
    };
    commit(&s, &headers, &f.csrf, vec![plan], &next).await
}

/// Replaces Atlas's copy of a same-named skill with this machine's ("Take theirs").
pub(crate) async fn take_theirs(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let Some(item) = snap.outside.skills.iter().find(|x| x.key == f.key) else {
        return refuse(&headers, GONE);
    };
    let Some(clash) = item.clash.as_ref().filter(|c| !c.same) else {
        return refuse(&headers, GONE);
    };
    if let Some(reason) = item.blocked() {
        return refuse(&headers, format!("{reason}; Atlas doesn't bring it in."));
    }
    let (kind, root, rel) = match &clash.home {
        SkillHome::Private => (
            RepoKind::Knowledge,
            s.sources.knowledge.clone(),
            format!("skills/{}", item.name),
        ),
        SkillHome::Public => match &s.sources.public_skills {
            Some(p) => (RepoKind::PublicSkills, p.clone(), item.name.clone()),
            None => return refuse(&headers, GONE),
        },
        SkillHome::Project(p) => match s.sources.projects.iter().find(|x| &x.name == p) {
            Some(x) => (
                RepoKind::Project(p.clone()),
                x.root.clone(),
                format!(".agents/skills/{}", item.name),
            ),
            None => return refuse(&headers, GONE),
        },
    };
    let tree = match read_source(&s, item) {
        Ok(t) => t,
        Err(m) => return refuse(&headers, m),
    };
    let plan = Plan {
        kind,
        root,
        message: format!(
            "Take {}'s copy of {} into Atlas",
            item.entries[0], item.name
        ),
        writes: vec![Write::Tree {
            rel,
            tree,
            replace: true,
        }],
    };
    commit(
        &s,
        &headers,
        &f.csrf,
        vec![plan],
        &with_done("/skills", "theirs"),
    )
    .await
}

/// Brings every plain outside skill into Global, as one commit.
pub(crate) async fn bring_all_skills(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let mut names = BTreeSet::new();
    let mut writes = Vec::new();
    let mut skipped = 0;
    for item in snap.outside.skills.iter().filter(|x| plain_import(x)) {
        if !names.insert(item.name.clone()) {
            continue;
        }
        let tree = match read_source(&s, item) {
            Ok(t) => t,
            Err(m) => return refuse(&headers, m),
        };
        skipped += tree.skipped.len();
        writes.push(Write::Tree {
            rel: format!("skills/{}", item.name),
            tree,
            replace: false,
        });
    }
    if writes.is_empty() {
        return change::respond(&s, &headers, &Outcome::Unchanged, "/skills");
    }
    let message = format!(
        "Bring {} into Atlas",
        plural(
            writes.len(),
            "skill from this machine",
            "skills from this machine"
        )
    );
    let next = if skipped > 0 {
        with_done("/skills", &format!("skipped-{skipped}"))
    } else {
        "/skills".to_owned()
    };
    commit(
        &s,
        &headers,
        &f.csrf,
        vec![knowledge(&s, writes, message)],
        &next,
    )
    .await
}

pub(crate) fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {many}")
    }
}

fn memory_next(snap: &Snapshot, scope: &str) -> String {
    match views::resolve_scope(snap, scope) {
        Scope::Global => "/memory".to_owned(),
        Scope::Project(p) => format!("/memory?scope={p}"),
    }
}

/// The writes that bring `files` of one project's Claude Code notes into the store:
/// each note byte for byte, and the index recording which machine they came from.
fn note_writes(
    s: &AppState,
    group: &outside::ProjectNotes,
    files: &[String],
) -> Result<Vec<Write>, String> {
    let tree = library::read_tree(&group.folder)
        .map_err(|e| format!("{} can't be read: {e}.", group.shown))?;
    let base = format!("memory/projects/{}", group.project);
    let mut writes = Vec::new();
    for file in files {
        let f = tree
            .files
            .iter()
            .find(|t| t.rel.to_str() == Some(file.as_str()))
            .ok_or_else(|| GONE.to_owned())?;
        writes.push(Write::File {
            rel: format!("{base}/{file}"),
            bytes: f.bytes.clone(),
        });
    }
    // A Claude Code folder with its own index.md keeps it; Atlas then records nothing.
    let own_index = group.notes.iter().any(|n| n.file == outside::NOTES_INDEX);
    if !own_index {
        let at = s.sources.knowledge.join(&base).join(outside::NOTES_INDEX);
        let existing = std::fs::symlink_metadata(&at)
            .ok()
            .filter(|m| m.is_file() && m.len() <= 1024 * 1024)
            .and_then(|_| std::fs::read_to_string(&at).ok());
        let index =
            outside::notes_index(existing.as_deref(), group.project.as_str(), &s.hub, files);
        if existing.as_deref() != Some(index.as_str()) {
            writes.push(Write::File {
                rel: format!("{base}/{}", outside::NOTES_INDEX),
                bytes: index.into_bytes(),
            });
        }
    }
    Ok(writes)
}

/// Brings one of Claude Code's notes into the store.
pub(crate) async fn bring_note(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let found = snap.outside.memory.iter().find_map(|g| {
        g.notes
            .iter()
            .find(|n| n.key == f.key && !n.kept)
            .map(|n| (g, n))
    });
    let Some((group, note)) = found else {
        return refuse(&headers, GONE);
    };
    let next = with_done(&memory_next(&snap, &f.scope), "notes");
    match note.state {
        NoteState::Same => return change::respond(&s, &headers, &Outcome::Unchanged, &next),
        NoteState::Differs if f.replace.is_none() => {
            return refuse(
                &headers,
                format!(
                    "Atlas has another {} for {}. Use this machine's copy, or keep Atlas's and remove this one by hand.",
                    note.file, group.project
                ),
            );
        }
        NoteState::New | NoteState::Differs => {}
    }
    let writes = match note_writes(&s, group, std::slice::from_ref(&note.file)) {
        Ok(w) => w,
        Err(m) => return refuse(&headers, m),
    };
    let message = format!(
        "Bring Claude Code's note {} on {} into Atlas",
        note.file, group.project
    );
    commit(
        &s,
        &headers,
        &f.csrf,
        vec![knowledge(&s, writes, message)],
        &next,
    )
    .await
}

/// Brings every new note in (one project's, on a project page), as one commit.
pub(crate) async fn bring_all_notes(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let scope = views::resolve_scope(&snap, &f.scope);
    let mut writes = Vec::new();
    let mut count = 0;
    for group in &snap.outside.memory {
        if scope.project().is_some_and(|p| p != &group.project) {
            continue;
        }
        let files: Vec<String> = group
            .notes
            .iter()
            .filter(|n| !n.kept && n.state == NoteState::New)
            .map(|n| n.file.clone())
            .collect();
        if files.is_empty() {
            continue;
        }
        count += files.len();
        match note_writes(&s, group, &files) {
            Ok(w) => writes.extend(w),
            Err(m) => return refuse(&headers, m),
        }
    }
    let next = with_done(&memory_next(&snap, &f.scope), "notes");
    if writes.is_empty() {
        return change::respond(&s, &headers, &Outcome::Unchanged, &next);
    }
    let message = format!(
        "Bring {} into Atlas",
        plural(count, "Claude Code note", "Claude Code notes")
    );
    commit(
        &s,
        &headers,
        &f.csrf,
        vec![knowledge(&s, writes, message)],
        &next,
    )
    .await
}

/// The writes for one hook: `hooks/<name>/hook.toml` and its script.
fn hook_writes(snap: &Snapshot, hook: &OutsideHook, name: &str) -> Result<Vec<Write>, String> {
    if let Some(reason) = hook_blocked(hook) {
        return Err(format!("{reason}; Atlas doesn't bring it in."));
    }
    let slug = Slug::new(name)
        .map_err(|_| "A hook's name is lowercase a-z, 0-9 and hyphens.".to_owned())?;
    let taken = snap
        .catalog
        .hooks
        .iter()
        .any(|h| h.scope == Scope::Global && h.name == slug);
    if taken {
        return Err(format!(
            "Atlas has a hook called {slug} already; pick another name."
        ));
    }
    let manifest = outside::hook_manifest(hook)?;
    let file = |rel: &str, text: String, executable: bool| library::TreeFile {
        rel: PathBuf::from(rel),
        bytes: text.into_bytes(),
        executable,
    };
    Ok(vec![Write::Tree {
        rel: format!("hooks/{slug}"),
        tree: Tree {
            files: vec![
                file("hook.toml", manifest, false),
                file(outside::HOOK_SCRIPT, outside::hook_script(hook), true),
            ],
            skipped: Vec::new(),
        },
        replace: false,
    }])
}

/// Brings one hook into Atlas under the name the owner gave it.
pub(crate) async fn bring_hook(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let Some(hook) = snap
        .outside
        .hooks
        .iter()
        .find(|h| h.key == f.key && !h.kept)
    else {
        return refuse(&headers, GONE);
    };
    let name = if f.name.trim().is_empty() {
        hook.name.clone()
    } else {
        f.name.trim().to_owned()
    };
    let writes = match hook_writes(&snap, hook, &name) {
        Ok(w) => w,
        Err(m) => return refuse(&headers, m),
    };
    let message = format!("Bring the hook {name} into Atlas from {}", hook.config);
    commit(
        &s,
        &headers,
        &f.csrf,
        vec![knowledge(&s, writes, message)],
        &with_done("/hooks", "hook"),
    )
    .await
}

/// Brings every hook that can come in, under their suggested names, as one commit.
pub(crate) async fn bring_all_hooks(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let mut writes = Vec::new();
    let mut count = 0;
    for hook in snap
        .outside
        .hooks
        .iter()
        .filter(|h| !h.kept && hook_blocked(h).is_none())
    {
        match hook_writes(&snap, hook, &hook.name) {
            Ok(w) => writes.extend(w),
            Err(m) => return refuse(&headers, m),
        }
        count += 1;
    }
    if writes.is_empty() {
        return change::respond(&s, &headers, &Outcome::Unchanged, "/hooks");
    }
    let message = format!(
        "Bring {} into Atlas",
        plural(count, "hook from this machine", "hooks from this machine")
    );
    commit(
        &s,
        &headers,
        &f.csrf,
        vec![knowledge(&s, writes, message)],
        &with_done("/hooks", "hook"),
    )
    .await
}

/// Where a key's page is, for going back after Leave it outside.
fn page_of(key: &str) -> &'static str {
    if key.starts_with("hook:") {
        "/hooks"
    } else if key.starts_with("memory:") {
        "/memory"
    } else {
        "/skills"
    }
}

/// Whether `key` names an item outside Atlas right now.
pub(crate) fn known_key(snap: &Snapshot, key: &str) -> bool {
    let o = &snap.outside;
    o.skills
        .iter()
        .any(|s| s.key == key || s.entries.iter().any(|e| format!("skill:{e}") == key))
        || o.hooks.iter().any(|h| h.key == key)
        || o.memory
            .iter()
            .any(|g| g.notes.iter().any(|n| n.key == key))
}

/// "Leave it outside": records the item in `[keep]`, so it isn't offered again.
pub(crate) async fn keep(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ItemForm>,
) -> Response {
    if let Some(r) = rejected(&s, &headers, &f.csrf) {
        return r;
    }
    let snap = match fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    if !known_key(&snap, &f.key) {
        return refuse(&headers, GONE);
    }
    let key = f.key.clone();
    let back = match page_of(&f.key) {
        "/memory" => memory_next(&snap, &f.scope),
        other => other.to_owned(),
    };
    let what = f.key.split_once(':').map_or(f.key.as_str(), |(_, w)| w);
    let what = what.split_once('#').map_or(what, |(w, _)| w);
    let message = format!("Leave {what} outside Atlas");
    let plan = knowledge(
        &s,
        vec![Write::Placements(Box::new(move |p| {
            p.keep.items.insert(key.clone());
        }))],
        message,
    );
    commit(&s, &headers, &f.csrf, vec![plan], &with_done(&back, "kept")).await
}

// ---------------------------------------------------------------- compare

#[derive(Debug)]
pub struct FileLine {
    pub name: String,
    /// `same`, `differs`, `left` (only in the first copy) or `right`.
    pub state: &'static str,
    pub label: &'static str,
}

#[derive(Debug)]
pub struct TextPair {
    pub name: String,
    pub left: String,
    pub right: String,
}

#[derive(Debug)]
pub struct Compare {
    pub left: String,
    pub right: String,
    pub files: Vec<FileLine>,
    pub texts: Vec<TextPair>,
    /// For an outside skill: the key Take theirs posts.
    pub key: Option<String>,
    pub back: String,
}

const MAX_SHOWN: usize = 64 * 1024;

fn shown_text(bytes: &[u8]) -> String {
    let cut = &bytes[..bytes.len().min(MAX_SHOWN)];
    let mut text = String::from_utf8_lossy(cut).into_owned();
    if bytes.len() > MAX_SHOWN {
        text.push_str("\n…");
    }
    text
}

/// Compares two folders' files: which are the same, which differ, which only one has, and
/// the text of up to three that differ.
pub(crate) fn compare_trees(left: &Tree, right: &Tree) -> (Vec<FileLine>, Vec<TextPair>) {
    let names: BTreeSet<&Path> = left
        .files
        .iter()
        .chain(&right.files)
        .map(|f| f.rel.as_path())
        .collect();
    let mut files = Vec::new();
    let mut texts = Vec::new();
    for name in names {
        let l = left.files.iter().find(|f| f.rel == name);
        let r = right.files.iter().find(|f| f.rel == name);
        let (state, label) = match (l, r) {
            (Some(a), Some(b)) if a.bytes == b.bytes => ("same", "Same"),
            (Some(a), Some(b)) => {
                if texts.len() < 3 {
                    texts.push(TextPair {
                        name: name.display().to_string(),
                        left: shown_text(&a.bytes),
                        right: shown_text(&b.bytes),
                    });
                }
                ("differs", "Differs")
            }
            (Some(_), None) => ("left", "Only Atlas's"),
            _ => ("right", "Only this machine's"),
        };
        files.push(FileLine {
            name: name.display().to_string(),
            state,
            label,
        });
    }
    (files, texts)
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CompareQuery {
    #[serde(default)]
    key: String,
}

#[derive(Template)]
#[template(path = "compare.html")]
struct ComparePage {
    page: Page,
    v: Compare,
}

/// Atlas's copy of a skill beside the outside one of the same name.
pub(crate) async fn compare(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CompareQuery>,
) -> Result<Html<String>, AppError> {
    let snap = s.snapshot().await?;
    let item = snap
        .outside
        .skills
        .iter()
        .find(|x| x.key == q.key)
        .ok_or(AppError::NotFound)?;
    let clash = item.clash.as_ref().ok_or(AppError::NotFound)?;
    let atlas = outside::skill_folder(&s.sources, &clash.home, &item.name)
        .and_then(|p| library::read_tree(&p).ok())
        .ok_or(AppError::NotFound)?;
    let theirs = read_source(&s, item).map_err(AppError::Bad)?;
    let (files, texts) = compare_trees(&atlas, &theirs);
    let page = s.page(
        &headers,
        "/skills",
        format!("Compare {}", item.name),
        &snap,
        &Scope::Global,
        false,
    );
    let home = match &clash.home {
        SkillHome::Private => "Global".to_owned(),
        SkillHome::Public => "Global, public".to_owned(),
        SkillHome::Project(p) => p.to_string(),
    };
    let v = Compare {
        left: format!("Atlas's copy ({home})"),
        right: format!("This machine's, {}", item.shown_real),
        files,
        texts,
        key: Some(item.key.clone()),
        back: "/skills".to_owned(),
    };
    Ok(Html(ComparePage { page, v }.render()?))
}

/// A project's copy of a skill beside the Global one, for Tidy up.
pub(crate) async fn compare_copies(
    s: &AppState,
    headers: &HeaderMap,
    project: &str,
    name: &str,
) -> Result<Html<String>, AppError> {
    let snap = s.snapshot().await?;
    let dup = snap
        .outside
        .duplicates
        .iter()
        .find(|d| d.project.as_str() == project && d.name.as_str() == name)
        .ok_or(AppError::NotFound)?;
    let read = |home: &SkillHome| {
        outside::skill_folder(&s.sources, home, name).and_then(|p| library::read_tree(&p).ok())
    };
    let global = read(&dup.global).ok_or(AppError::NotFound)?;
    let local = read(&SkillHome::Project(dup.project.clone())).ok_or(AppError::NotFound)?;
    let (files, texts) = compare_trees(&global, &local);
    let page = s.page(
        headers,
        "/skills",
        format!("Compare {name}"),
        &snap,
        &Scope::Global,
        false,
    );
    let v = Compare {
        left: "Global's copy".to_owned(),
        right: format!("{project}'s copy"),
        files: files
            .into_iter()
            .map(|mut f| {
                f.label = match f.state {
                    "left" => "Only Global's",
                    "right" => "Only the project's",
                    _ => f.label,
                };
                f
            })
            .collect(),
        texts,
        key: None,
        back: "/skills/tidy".to_owned(),
    };
    Ok(Html(ComparePage { page, v }.render()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_codes_read_as_sentences() {
        assert!(
            done_text("skipped-2")
                .unwrap()
                .starts_with("2 entries inside the folder")
        );
        assert!(done_text("already").is_some());
        assert_eq!(done_text("<script>"), None);
    }

    #[test]
    fn comparing_trees_names_each_file() {
        let tree = |files: &[(&str, &str)]| Tree {
            files: files
                .iter()
                .map(|(n, b)| library::TreeFile {
                    rel: PathBuf::from(n),
                    bytes: b.as_bytes().to_vec(),
                    executable: false,
                })
                .collect(),
            skipped: Vec::new(),
        };
        let (files, texts) = compare_trees(
            &tree(&[("SKILL.md", "a"), ("x.sh", "1"), ("old", "o")]),
            &tree(&[("SKILL.md", "b"), ("x.sh", "1"), ("new", "n")]),
        );
        let states: Vec<(&str, &str)> = files.iter().map(|f| (f.name.as_str(), f.state)).collect();
        assert_eq!(
            states,
            [
                ("SKILL.md", "differs"),
                ("new", "right"),
                ("old", "left"),
                ("x.sh", "same")
            ]
        );
        assert_eq!(texts.len(), 1);
        assert_eq!(
            (texts[0].left.as_str(), texts[0].right.as_str()),
            ("a", "b")
        );
    }
}
