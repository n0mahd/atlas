//! The Instructions pages and the changes they offer: who reads each file (from
//! `atlas_harness::readers`, applied to what sits on the hub's own machine), "Use Global
//! instead" for an agent's own home file, moving or merging a project's `CLAUDE.md` into
//! `AGENTS.md`, "Move to Global", "Copy to this project", a new project file (a new
//! `CLAUDE.local.md` is kept out of git), and Claude Code's AGENTS.md option.
//!
//! Every change is one [`change::commit_change`]: the files it reads are fingerprinted when
//! the confirmation is shown and again under the git lock, so a file edited meanwhile is
//! never overwritten. Only the instruction files named in `readers` (and the placements
//! file) are ever written, each read within 1 MB. Text moved between files goes through
//! [`append_section`]: appended after a blank line and an HTML comment naming where it
//! came from, and only when the target doesn't already hold every line of it.
//!
//! The hub sees only its own machine's home files. "Use Global instead" adds the text to
//! Global; atlasd then replaces the agent's file on each machine only where every line of
//! it is in Global (`atlas_harness::instructions::contained`).
//!
//! Facts about this machine ([`read_facts`]) cover the agents' home files and one
//! project at most, the one shown or changed, since a project's facts can cost git runs
//! (through `atlas_git`, never a bare `git`). The pages keep them with the snapshot
//! ([`FactsSlot`]); a change checks the session first, then reads them fresh.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use askama::Template;
use atlas_core::catalog::edit::RepoKind;
use atlas_core::catalog::{ClaudeAgentsMd, PLACEMENTS_FILE};
use atlas_core::kernel::{HarnessKind, Slug};
use atlas_fs::Sources;
use atlas_harness::instructions::{
    append_section, claude_choice, contained, imports_path, read_claude_agents_md,
    without_imports_of,
};
use atlas_harness::readers::{
    Access, FileFacts, HomeFile, Present, Why, project_access, readers_of,
};
use axum::extract::{Form, Path as UrlPath, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::change::{self, ChangeError, Outcome, RepoChange};
use crate::views::{self, Page};
use crate::{AppError, AppState, ListQuery, PageResult};

/// The biggest instruction file Atlas reads or writes.
pub const MAX_BYTES: u64 = 1024 * 1024;

/// Project files Atlas can create, move or merge. Every one is a name agents read.
pub const PROJECT_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "CLAUDE.local.md"];

/// atlasd's own copy of the knowledge repo on a machine (`atlas-client` `Paths`). On the
/// hub's machine a link or import pointing there reads Global too.
const ATLASD_GLOBAL: &str = ".local/share/atlas/sources/knowledge/AGENTS.md";

// ---------------------------------------------------------------- facts on this machine

/// Claude Code's AGENTS.md option: the owner's choice in Atlas and what this machine has.
#[derive(Clone, Debug, Default)]
pub struct ClaudeSetting {
    pub placed: Option<ClaudeAgentsMd>,
    /// Claude Code's own word in `~/.claude/settings.json`, if set.
    pub machine: Option<String>,
}

impl ClaudeSetting {
    /// What Claude does here once atlasd has applied the owner's choice.
    #[must_use]
    pub fn effective(&self) -> ClaudeAgentsMd {
        self.placed
            .or_else(|| self.machine.as_deref().and_then(claude_choice))
            .unwrap_or_default()
    }
}

/// One agent's home instruction files on this machine.
#[derive(Clone, Debug, Default)]
pub struct AgentHome {
    /// What sits at each of its home files (and the other agents' files it reads), by
    /// path relative to the home folder.
    pub files: BTreeMap<String, HomeFile>,
    /// The file Atlas links or imports Global into, shown with `~`.
    pub shown: String,
    /// Its text when that file is the agent's own (or, for Claude, has text besides the
    /// import): what "Use Global instead" moves.
    pub own_text: Option<String>,
}

/// A project's instruction files in its top folder.
#[derive(Clone, Debug, Default)]
pub struct ProjectFacts {
    pub present: Present,
    /// `CLAUDE.md` only links to or imports `AGENTS.md`, so it isn't a file of its own.
    pub claude_points_at_agents: bool,
    /// `CLAUDE.local.md`'s text (not in the catalog: it isn't in git).
    pub local_text: Option<String>,
}

/// What the Instructions pages know about the hub's own machine.
#[derive(Clone, Debug, Default)]
pub struct InstructionFacts {
    /// Agents set up on this machine.
    pub homes: BTreeMap<HarnessKind, AgentHome>,
    pub projects: BTreeMap<Slug, ProjectFacts>,
    pub claude: ClaudeSetting,
}

/// A small file's text, refusing links and anything over [`MAX_BYTES`]. `Ok(None)`: missing.
fn read_text(path: &Path) -> Result<Option<String>, String> {
    let meta = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
        Ok(m) => m,
    };
    if meta.file_type().is_symlink() {
        return Err("it is a link; Atlas changes files only where they really are".to_owned());
    }
    if !meta.is_file() {
        return Err("it isn't a regular file".to_owned());
    }
    if meta.len() > MAX_BYTES {
        return Err("it is larger than 1 MB".to_owned());
    }
    let mut text = String::new();
    fs::File::open(path)
        .and_then(|f| f.take(MAX_BYTES + 1).read_to_string(&mut text))
        .map_err(|e| e.to_string())?;
    Ok(Some(text))
}

fn globals(sources: &Sources) -> Vec<PathBuf> {
    [
        sources.knowledge.join("AGENTS.md"),
        sources.home.join(ATLASD_GLOBAL),
    ]
    .iter()
    .filter_map(|p| fs::canonicalize(p).ok())
    .collect()
}

fn is_global(path: &Path, globals: &[PathBuf]) -> bool {
    fs::canonicalize(path).is_ok_and(|c| globals.contains(&c))
}

/// `text` without its lines importing Global by any path [`globals`] knows (the knowledge
/// repo's `AGENTS.md`, atlasd's copy, a link to either): moved into Global, such a line
/// would make Global import itself.
fn without_global_imports(text: &str, dir: &Path, home: &Path, globals: &[PathBuf]) -> String {
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !imports_path(l, dir, home).is_some_and(|p| is_global(&p, globals)))
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() && text.ends_with('\n') {
        out.push('\n');
    }
    out
}

fn home_file(
    sources: &Sources,
    path: &Path,
    globals: &[PathBuf],
) -> Option<(HomeFile, Option<String>)> {
    let meta = fs::symlink_metadata(path).ok()?;
    if meta.file_type().is_symlink() {
        let kind = if is_global(path, globals) {
            HomeFile::LinksGlobal
        } else {
            HomeFile::Other
        };
        return Some((kind, None));
    }
    let Ok(Some(text)) = read_text(path) else {
        return Some((HomeFile::Other, None));
    };
    let dir = path.parent().unwrap_or(&sources.home);
    let import_of_global =
        |l: &str| imports_path(l, dir, &sources.home).is_some_and(|p| is_global(&p, globals));
    if text.lines().any(import_of_global) {
        let own = text
            .lines()
            .any(|l| !l.trim().is_empty() && !import_of_global(l));
        return Some((HomeFile::ImportsGlobal { own }, own.then_some(text)));
    }
    Some((HomeFile::Own, Some(text)))
}

fn project_facts(sources: &Sources, root: &Path) -> ProjectFacts {
    let mut facts = ProjectFacts::default();
    let canonical_root = fs::canonicalize(root).ok();
    let names = atlas_harness::readers::project_names();
    // Opened only when a local file is there to ask git about.
    let mut repo: Option<Option<atlas_git::Repo>> = None;
    for name in &names {
        let path = root.join(name);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let mut f = FileFacts::default();
        if meta.file_type().is_symlink() {
            // A link to another instruction name in the same folder.
            f.links_to = fs::canonicalize(&path).ok().and_then(|c| {
                (c.parent() == canonical_root.as_deref())
                    .then(|| c.file_name()?.to_str().map(str::to_owned))
                    .flatten()
                    .filter(|n| names.contains(n.as_str()))
            });
        } else if let Ok(Some(text)) = read_text(&path) {
            for line in text.lines() {
                if let Some(p) = imports_path(line, root, &sources.home)
                    && p.parent() == Some(root)
                    && let Some(n) = p.file_name().and_then(|n| n.to_str())
                    && names.contains(n)
                {
                    f.imports.insert(n.to_owned());
                }
            }
            if *name == "CLAUDE.md" {
                let agents = root.join("AGENTS.md");
                facts.claude_points_at_agents = text.lines().all(|l| {
                    l.trim().is_empty()
                        || imports_path(l, root, &sources.home).is_some_and(|p| p == agents)
                }) && !text.trim().is_empty();
            }
            if *name == "CLAUDE.local.md" {
                facts.local_text = Some(text);
            }
        }
        if name.contains("local") {
            let repo = repo.get_or_insert_with(|| atlas_git::Repo::open(root).ok());
            f.ignored = repo
                .as_ref()
                .is_some_and(|r| r.is_ignored(Path::new(name)).unwrap_or(false));
        }
        if *name == "CLAUDE.md" && f.links_to.as_deref() == Some("AGENTS.md") {
            facts.claude_points_at_agents = true;
        }
        facts.present.insert((*name).to_owned(), f);
    }
    facts
}

/// Reads what the Instructions pages show about this machine: the agents' home files,
/// Claude's AGENTS.md option, and the files of `project` only (none for Global), since
/// a project's facts cost a git run. Blocking. `placed` is the owner's Claude choice from
/// the snapshot's placements.
#[must_use]
pub fn read_facts(
    sources: &Sources,
    placed: Option<ClaudeAgentsMd>,
    project: Option<&Slug>,
) -> InstructionFacts {
    let globals = globals(sources);
    let mut out = InstructionFacts::default();
    for layout in atlas_harness::layouts() {
        let Some(home) = readers_of(layout.kind).and_then(|r| r.home) else {
            continue;
        };
        if !sources.home.join(layout.marker).exists() {
            continue;
        }
        let mut agent = AgentHome {
            shown: sources.display(&sources.home.join(home.shared)),
            ..AgentHome::default()
        };
        let also = home.also_reads.iter().map(|a| a.file);
        for rel in home.files.iter().copied().chain(also) {
            if let Some((kind, text)) = home_file(sources, &sources.home.join(rel), &globals) {
                if rel == home.shared
                    && matches!(kind, HomeFile::Own | HomeFile::ImportsGlobal { own: true })
                {
                    agent.own_text = text;
                }
                agent.files.insert(rel.to_owned(), kind);
            }
        }
        out.homes.insert(layout.kind, agent);
    }
    for p in sources.projects.iter().filter(|p| Some(&p.name) == project) {
        out.projects
            .insert(p.name.clone(), project_facts(sources, &p.root));
    }
    out.claude.placed = placed;
    let settings = sources.home.join(".claude/settings.json");
    if let Ok(Some(text)) = read_text(&settings) {
        out.claude.machine = read_claude_agents_md(Some(text.as_bytes()));
    }
    out
}

/// [`read_facts`] results kept with one snapshot ([`AppState::snapshot`]'s cache), by the
/// project they cover (`None`: Global only), so the pages read them once per snapshot and
/// a save, which drops the snapshot, drops them too.
pub(crate) type FactsSlot = Arc<tokio::sync::Mutex<BTreeMap<Option<Slug>, Arc<InstructionFacts>>>>;

/// The snapshot and the facts for the scope `scope_key` names, read once per snapshot.
async fn facts(
    s: &AppState,
    scope_key: &str,
) -> Result<(Arc<views::Snapshot>, Arc<InstructionFacts>), AppError> {
    let (snap, slot) = s.cached().await?;
    let project = views::resolve_scope(&snap, scope_key).project().cloned();
    let mut slot = slot.lock().await;
    if let Some(f) = slot.get(&project) {
        return Ok((snap, Arc::clone(f)));
    }
    let sources = Arc::clone(&s.sources);
    let placed = snap.catalog.placements.instructions.claude_reads_agents_md;
    let p = project.clone();
    let f = tokio::task::spawn_blocking(move || read_facts(&sources, placed, p.as_ref()))
        .await
        .map(Arc::new)
        .map_err(|_| AppError::Internal)?;
    slot.insert(project, Arc::clone(&f));
    Ok((snap, f))
}

// ---------------------------------------------------------------- pages

#[derive(Template)]
#[template(path = "instructions.html")]
struct ListPage {
    page: Page,
    v: views::InstructionsList,
}

#[derive(Template)]
#[template(path = "instruction.html")]
struct DetailPage {
    page: Page,
    v: views::InstructionDetail,
}

#[derive(Template)]
#[template(path = "instruction_confirm.html")]
struct ConfirmPage {
    page: Page,
    c: Confirm,
    send_on_save: bool,
}

/// What a confirmation shows before a change.
#[derive(Debug, Default)]
pub struct Confirm {
    pub action: String,
    pub back: String,
    pub lines: Vec<String>,
    /// The text that will be added somewhere, shown as it will read.
    pub preview: String,
    pub undo: String,
    pub button: String,
    pub base: String,
    /// Offer turning Claude's AGENTS.md option to "alongside", with this reason.
    pub offer_claude: String,
    /// A new file: the name and a text box.
    pub create: bool,
    /// The change commits to the knowledge repo, so Save may send it.
    pub sends: bool,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Extra {
    /// After a new `CLAUDE.local.md`: it was made, outside git.
    #[serde(default)]
    local: Option<String>,
    /// After moving `CLAUDE.local.md` to Global: its text is in Global but the file stayed.
    #[serde(default)]
    kept: Option<String>,
}

fn extra_flash(page: &mut Page, extra: &Extra) {
    let text = if extra.local.is_some() {
        "Made CLAUDE.local.md. It is kept out of git, so git can't undo it; delete the file to undo."
    } else if extra.kept.is_some() {
        "Its text is in Global now, but the project file changed meanwhile, so it was kept."
    } else {
        return;
    };
    text.clone_into(&mut page.flash);
}

pub(crate) async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> PageResult {
    let (snap, facts) = facts(&s, &q.scope).await?;
    let scope = views::resolve_scope(&snap, &q.scope);
    let v = views::instructions(&snap, &scope, &facts);
    let mut page = s.page(
        &headers,
        "/instructions",
        "Instructions",
        &snap,
        &scope,
        true,
    );
    q.apply(&mut page);
    page.totals = format!("{} files", v.files);
    Ok(Html(ListPage { page, v }.render()?))
}

pub(crate) async fn detail(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath((scope_key, file)): UrlPath<(String, String)>,
    Query(q): Query<ListQuery>,
    Query(extra): Query<Extra>,
) -> PageResult {
    let (snap, facts) = facts(&s, &scope_key).await?;
    let v =
        views::instruction_detail(&snap, &scope_key, &file, &facts).ok_or(AppError::NotFound)?;
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
    extra_flash(&mut page, &extra);
    page.edit_href = (s.writer.is_some() && v.editable)
        .then(|| format!("/instructions/{scope_key}/{file}/edit"));
    Ok(Html(DetailPage { page, v }.render()?))
}

// ---------------------------------------------------------------- the changes

#[derive(Clone, Debug, PartialEq, Eq)]
enum Op {
    /// An agent's own home file: its text goes into Global.
    UseGlobal(HarnessKind),
    /// A project's `CLAUDE.md` becomes (or merges into) its `AGENTS.md`.
    ToAgents(Slug),
    /// A project file's text goes to the end of Global, and the file goes.
    ToGlobal(Slug, &'static str),
    /// Global's text goes to the end of the project's `AGENTS.md`.
    FromGlobal(Slug),
    /// A new file in the project.
    Create(Slug, &'static str),
}

fn project_root(sources: &Sources, scope: &str) -> Option<(Slug, PathBuf)> {
    sources
        .projects
        .iter()
        .find(|p| p.name.as_str() == scope)
        .map(|p| (p.name.clone(), p.root.clone()))
}

fn known_file(file: &str) -> Option<&'static str> {
    PROJECT_FILES.iter().copied().find(|f| *f == file)
}

fn parse(sources: &Sources, scope: &str, file: &str, action: &str) -> Option<Op> {
    if scope == "global" {
        let kind: HarnessKind = file.parse().ok()?;
        readers_of(kind)?.home?;
        return (action == "use-global").then_some(Op::UseGlobal(kind));
    }
    let (project, _) = project_root(sources, scope)?;
    let file = known_file(file)?;
    match action {
        "to-agents" if file == "CLAUDE.md" => Some(Op::ToAgents(project)),
        "to-global" => Some(Op::ToGlobal(project, file)),
        "from-global" if file == "AGENTS.md" => Some(Op::FromGlobal(project)),
        "create" => Some(Op::Create(project, file)),
        _ => None,
    }
}

/// A file a change writes or removes.
#[derive(Clone, Debug)]
enum Write {
    Put {
        root: PathBuf,
        rel: &'static str,
        text: String,
    },
    Remove {
        root: PathBuf,
        rel: &'static str,
    },
    /// Atlas's block in `.git/info/exclude` gets `/<rel>`.
    Exclude {
        root: PathBuf,
        rel: &'static str,
    },
    ClaudeAlongside,
}

/// A file removed once Global holds its text.
struct RemoveAfter {
    path: PathBuf,
    /// The file as the change read it; a file edited since is kept.
    text: String,
    /// What Global gained from it: `text` without its imports of Global.
    own: String,
}

/// A change worked out from the files as they are now.
struct Planned {
    confirm: Confirm,
    repos: Vec<RepoChange>,
    writes: Vec<Write>,
    /// Files whose text the change depends on, fingerprinted in `confirm.base`.
    inputs: Vec<PathBuf>,
    /// `CLAUDE.local.md` moved to Global: removed after Global's commit, when its text is
    /// there (git can't restore it, so it never goes before that).
    remove_after: Option<RemoveAfter>,
    next: String,
}

fn fingerprint(inputs: &[PathBuf]) -> Result<String, String> {
    let mut all = String::new();
    for p in inputs {
        let text = read_text(p).map_err(|e| format!("{}: {e}", p.display()))?;
        match text {
            Some(t) => all.push_str(&atlas_fs::edit::content_hash(&t)),
            None => all.push('-'),
        }
        all.push(';');
    }
    Ok(atlas_fs::edit::content_hash(&all))
}

fn need(text: Result<Option<String>, String>, shown: &str) -> Result<String, String> {
    match text {
        Ok(Some(t)) => Ok(t),
        Ok(None) => Err(format!("{shown} doesn't exist.")),
        Err(e) => Err(format!("{shown} can't be used: {e}.")),
    }
}

fn check_size(text: &str, shown: &str) -> Result<(), String> {
    if text.len() as u64 > MAX_BYTES {
        return Err(format!("{shown} would be larger than 1 MB."));
    }
    Ok(())
}

fn git_note(project: &str) -> String {
    format!("One commit in {project}; git can undo it.")
}

fn plan(
    sources: &Sources,
    facts: &InstructionFacts,
    op: &Op,
    create_text: &str,
) -> Result<Planned, String> {
    let mut planned = match op {
        Op::UseGlobal(kind) => plan_use_global(sources, facts, *kind)?,
        Op::ToAgents(project) => plan_to_agents(sources, facts, project)?,
        Op::ToGlobal(project, file) => plan_to_global(sources, project, file)?,
        Op::FromGlobal(project) => plan_from_global(sources, project)?,
        Op::Create(project, file) => plan_create(sources, project, file, create_text)?,
    };
    planned.confirm.base = fingerprint(&planned.inputs)?;
    Ok(planned)
}

fn root_of(sources: &Sources, project: &Slug) -> Result<PathBuf, String> {
    project_root(sources, project.as_str())
        .map(|(_, root)| root)
        .ok_or_else(|| "That project isn't registered.".to_owned())
}

fn global_text(global: &Path) -> Result<String, String> {
    Ok(read_text(global)
        .map_err(|e| format!("Global can't be used: {e}."))?
        .unwrap_or_default())
}

fn knowledge_change(sources: &Sources, message: String) -> RepoChange {
    RepoChange {
        kind: RepoKind::Knowledge,
        root: sources.knowledge.clone(),
        paths: vec![PathBuf::from("AGENTS.md")],
        message,
    }
}

fn project_change(root: &Path, project: &Slug, paths: &[&str], message: String) -> RepoChange {
    RepoChange {
        kind: RepoKind::Project(project.clone()),
        root: root.to_path_buf(),
        paths: paths.iter().map(PathBuf::from).collect(),
        message,
    }
}

fn plan_use_global(
    sources: &Sources,
    facts: &InstructionFacts,
    kind: HarnessKind,
) -> Result<Planned, String> {
    let global = sources.knowledge.join("AGENTS.md");
    let agent = facts
        .homes
        .get(&kind)
        .ok_or("That agent isn't set up on this machine.")?;
    let own = agent
        .own_text
        .clone()
        .ok_or_else(|| format!("{} has no text of its own.", agent.shown))?;
    let home = readers_of(kind)
        .and_then(|r| r.home)
        .ok_or("That agent has no home file.")?;
    let path = sources.home.join(home.shared);
    let dir = path.parent().unwrap_or(&sources.home);
    let own = without_global_imports(&own, dir, &sources.home, &globals(sources));
    let current = global_text(&global)?;
    let joined = append_section(&current, &agent.shown, &own);
    check_size(&joined, "Global")?;
    let first = if joined == current {
        format!(
            "Every line of {} is already in Global, so Global stays as it is.",
            agent.shown
        )
    } else {
        format!("{}'s text is added to the end of Global.", agent.shown)
    };
    let joins = if kind == HarnessKind::ClaudeCode {
        "import"
    } else {
        "link"
    };
    Ok(Planned {
        confirm: Confirm {
            lines: vec![
                first,
                format!(
                    "Then on each machine atlasd replaces {} with Global's {joins}, but only where every line of that machine's file is in Global. Any other file stays and is reported.",
                    agent.shown
                ),
            ],
            preview: own,
            undo: "One commit in your knowledge repo; git can undo it. Machines keep the agent's file until the update arrives.".to_owned(),
            button: "Use Global instead".to_owned(),
            sends: true,
            ..Confirm::default()
        },
        repos: vec![knowledge_change(
            sources,
            format!("Add {}'s instructions to Global", agent.shown),
        )],
        writes: vec![Write::Put {
            root: sources.knowledge.clone(),
            rel: "AGENTS.md",
            text: joined,
        }],
        inputs: vec![path, global],
        remove_after: None,
        next: "/instructions".to_owned(),
    })
}

fn plan_to_agents(
    sources: &Sources,
    facts: &InstructionFacts,
    project: &Slug,
) -> Result<Planned, String> {
    let root = root_of(sources, project)?;
    let claude_md = root.join("CLAUDE.md");
    let agents_md = root.join("AGENTS.md");
    let text = need(read_text(&claude_md), "CLAUDE.md")?;
    let pf = facts.projects.get(project).cloned().unwrap_or_default();
    if pf.claude_points_at_agents {
        return Err("CLAUDE.md only points at AGENTS.md already.".to_owned());
    }
    let agents = read_text(&agents_md).map_err(|e| format!("AGENTS.md can't be used: {e}."))?;
    // A line importing AGENTS.md would only point AGENTS.md at itself.
    let own = without_imports_of(&text, &agents_md, &root, &sources.home);
    let (joined, merge) = match &agents {
        Some(a) => (append_section(a, "CLAUDE.md", &own), true),
        None => (own.clone(), false),
    };
    check_size(&joined, "AGENTS.md")?;
    // Would Claude still skip AGENTS.md afterwards?
    let mut after = pf.present.clone();
    after.remove("CLAUDE.md");
    after.entry("AGENTS.md".to_owned()).or_default();
    let offer_claude = match project_access(
        HarnessKind::ClaudeCode,
        "AGENTS.md",
        &after,
        facts.claude.effective(),
    ) {
        Access::None(Why::ClaudeFiles(f)) => {
            format!("Claude would still skip AGENTS.md here while {f} is there.")
        }
        Access::None(_) => {
            "Claude would still skip AGENTS.md: its AGENTS.md option is off.".to_owned()
        }
        _ => String::new(),
    };
    Ok(Planned {
        confirm: Confirm {
            lines: vec![if merge {
                "CLAUDE.md's text is added to the end of AGENTS.md, then CLAUDE.md is deleted."
                    .to_owned()
            } else {
                "CLAUDE.md becomes AGENTS.md, so every agent reads it.".to_owned()
            }],
            preview: own,
            undo: git_note(project.as_str()),
            button: if merge {
                "Merge into AGENTS.md"
            } else {
                "Make it AGENTS.md"
            }
            .to_owned(),
            // Ticking the offer commits atlas/placements.toml in the knowledge repo, which
            // Save may then send; `change::respond` asks only when that commit happened.
            sends: !offer_claude.is_empty(),
            offer_claude,
            ..Confirm::default()
        },
        repos: vec![project_change(
            &root,
            project,
            &["AGENTS.md", "CLAUDE.md"],
            if merge {
                "Merge CLAUDE.md into AGENTS.md in Atlas".to_owned()
            } else {
                "Rename CLAUDE.md to AGENTS.md in Atlas".to_owned()
            },
        )],
        writes: vec![
            Write::Put {
                root: root.clone(),
                rel: "AGENTS.md",
                text: joined,
            },
            Write::Remove {
                root,
                rel: "CLAUDE.md",
            },
        ],
        inputs: vec![claude_md, agents_md],
        remove_after: None,
        next: format!("/instructions/{project}/AGENTS.md"),
    })
}

fn plan_to_global(
    sources: &Sources,
    project: &Slug,
    file: &'static str,
) -> Result<Planned, String> {
    let root = root_of(sources, project)?;
    let global = sources.knowledge.join("AGENTS.md");
    let path = root.join(file);
    let text = need(read_text(&path), file)?;
    let current = global_text(&global)?;
    let own = without_global_imports(&text, &root, &sources.home, &globals(sources));
    let from = format!("{project}/{file}");
    let joined = append_section(&current, &from, &own);
    check_size(&joined, "Global")?;
    let local = file == "CLAUDE.local.md";
    let mut repos = vec![knowledge_change(sources, format!("Add {from} to Global"))];
    let mut writes = vec![Write::Put {
        root: sources.knowledge.clone(),
        rel: "AGENTS.md",
        text: joined,
    }];
    let mut remove_after = None;
    if local {
        remove_after = Some(RemoveAfter {
            path: path.clone(),
            text,
            own: own.clone(),
        });
    } else {
        // Global gains the text first, so a failure part way leaves a copy, not a loss.
        repos.push(project_change(
            &root,
            project,
            &[file],
            format!("Move {file} to Global in Atlas"),
        ));
        writes.push(Write::Remove { root, rel: file });
    }
    Ok(Planned {
        confirm: Confirm {
            lines: vec![format!(
                "{file}'s text is added to the end of Global, then {file} is deleted from {project}."
            )],
            preview: own,
            undo: if local {
                "Global's change is one commit and git can undo it. CLAUDE.local.md isn't in git, so git can't bring it back once it's deleted; its text stays in Global.".to_owned()
            } else {
                format!("Two commits, Global first, then {project}; git can undo both.")
            },
            button: "Move to Global".to_owned(),
            sends: true,
            ..Confirm::default()
        },
        repos,
        writes,
        inputs: vec![path, global],
        remove_after,
        next: "/instructions/global/AGENTS.md".to_owned(),
    })
}

fn plan_from_global(sources: &Sources, project: &Slug) -> Result<Planned, String> {
    let root = root_of(sources, project)?;
    let global = sources.knowledge.join("AGENTS.md");
    let agents_md = root.join("AGENTS.md");
    let text = need(read_text(&global), "Global")?;
    let current = read_text(&agents_md)
        .map_err(|e| format!("AGENTS.md can't be used: {e}."))?
        .unwrap_or_default();
    let joined = append_section(&current, "Global AGENTS.md", &text);
    check_size(&joined, "AGENTS.md")?;
    Ok(Planned {
        confirm: Confirm {
            lines: vec![if joined == current {
                format!("{project}'s AGENTS.md already has every line of Global.")
            } else {
                format!(
                    "Global's text is added to the end of {project}'s AGENTS.md. Global stays as it is."
                )
            }],
            preview: text,
            undo: git_note(project.as_str()),
            button: "Copy to this project".to_owned(),
            ..Confirm::default()
        },
        repos: vec![project_change(
            &root,
            project,
            &["AGENTS.md"],
            "Copy Global's instructions into AGENTS.md in Atlas".to_owned(),
        )],
        writes: vec![Write::Put {
            root,
            rel: "AGENTS.md",
            text: joined,
        }],
        inputs: vec![global, agents_md],
        remove_after: None,
        next: format!("/instructions/{project}/AGENTS.md"),
    })
}

fn plan_create(
    sources: &Sources,
    project: &Slug,
    file: &'static str,
    text: &str,
) -> Result<Planned, String> {
    let root = root_of(sources, project)?;
    let path = root.join(file);
    if fs::symlink_metadata(&path).is_ok() {
        return Err(format!("{project} already has {file}."));
    }
    let text = atlas_core::catalog::edit::prepare(text, true).map_err(|e| e.to_string())?;
    let local = file == "CLAUDE.local.md";
    if local && !root.join(".git").is_dir() {
        return Err("This checkout's .git isn't a folder (a worktree?), so Atlas can't keep CLAUDE.local.md out of git here.".to_owned());
    }
    let mut writes = Vec::new();
    if local {
        // Kept out of git before it exists, so git never sees it.
        writes.push(Write::Exclude {
            root: root.clone(),
            rel: file,
        });
    }
    writes.push(Write::Put {
        root: root.clone(),
        rel: file,
        text,
    });
    Ok(Planned {
        confirm: Confirm {
            lines: vec![format!("A new {file} in {project}.")],
            undo: if local {
                "CLAUDE.local.md is kept out of git (in .git/info/exclude), so git can't undo it; delete the file to undo.".to_owned()
            } else {
                git_note(project.as_str())
            },
            button: "Create".to_owned(),
            create: true,
            ..Confirm::default()
        },
        repos: vec![project_change(
            &root,
            project,
            &[file],
            format!("Add {file} in Atlas"),
        )],
        writes,
        inputs: vec![path],
        remove_after: None,
        next: if local {
            format!("/instructions/{project}/{file}?local=1")
        } else {
            format!("/instructions/{project}/{file}")
        },
    })
}

fn apply_writes(writes: &[Write], knowledge: &Path) -> Result<(), String> {
    for w in writes {
        match w {
            Write::Put { root, rel, text } => {
                atlas_fs::library::write_file(root, rel, text.as_bytes())
                    .map_err(|e| format!("Couldn't write {rel}: {e}."))?;
            }
            Write::Remove { root, rel } => {
                let path = root.join(rel);
                let meta = fs::symlink_metadata(&path).map_err(|e| format!("{rel}: {e}."))?;
                if !meta.is_file() {
                    return Err(format!("{rel} isn't a plain file."));
                }
                fs::remove_file(&path).map_err(|e| format!("Couldn't delete {rel}: {e}."))?;
            }
            Write::Exclude { root, rel } => {
                let pattern = format!("/{rel}");
                atlas_fs::library::add_to_git_exclude(root, &[pattern.as_str()])
                    .map_err(|e| format!("Couldn't keep {rel} out of git: {e}."))?;
            }
            Write::ClaudeAlongside => set_claude(knowledge, ClaudeAgentsMd::Alongside)?,
        }
    }
    Ok(())
}

fn set_claude(knowledge: &Path, choice: ClaudeAgentsMd) -> Result<(), String> {
    let (mut p, base) = atlas_fs::library::read_placements(knowledge).map_err(|e| e.to_string())?;
    if p.instructions.claude_reads_agents_md == Some(choice) {
        return Ok(());
    }
    p.instructions.claude_reads_agents_md = Some(choice);
    atlas_fs::library::write_placements(knowledge, &p, &base).map_err(|e| e.to_string())?;
    Ok(())
}

#[derive(Debug, Deserialize)]
pub(crate) struct ChangeForm {
    csrf: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    text: String,
    /// Also turn Claude's AGENTS.md option to "alongside".
    #[serde(default)]
    claude: Option<String>,
}

async fn planned(s: &AppState, op: Op, text: String) -> Result<Planned, AppError> {
    let sources = Arc::clone(&s.sources);
    let placed = s
        .snapshot()
        .await?
        .catalog
        .placements
        .instructions
        .claude_reads_agents_md;
    tokio::task::spawn_blocking(move || {
        // Read fresh, not from the snapshot's cache: the plan moves this text. Only
        // CLAUDE.md to AGENTS.md needs a project's facts.
        let project = match &op {
            Op::ToAgents(p) => Some(p),
            _ => None,
        };
        let facts = read_facts(&sources, placed, project);
        plan(&sources, &facts, &op, &text)
    })
    .await
    .map_err(|_| AppError::Internal)?
    .map_err(AppError::Bad)
}

pub(crate) async fn confirm(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath((scope, file, action)): UrlPath<(String, String, String)>,
) -> Result<Response, AppError> {
    let op = parse(&s.sources, &scope, &file, &action).ok_or(AppError::NotFound)?;
    let snap = s.snapshot().await?;
    let mut page = s.page(
        &headers,
        "/instructions",
        title_of(&op),
        &snap,
        &views::resolve_scope(&snap, &scope),
        false,
    );
    if s.writer.is_none() {
        return Err(AppError::NotFound);
    }
    if !page.signed_in {
        return Ok(Redirect::to(&crate::auth::signin_href(&format!(
            "/instructions/{scope}/{file}/{action}"
        )))
        .into_response());
    }
    let p = planned(&s, op, String::new()).await?;
    let mut c = p.confirm;
    c.action = format!("/instructions/{scope}/{file}/{action}");
    c.back = if scope == "global" {
        "/instructions".to_owned()
    } else {
        format!("/instructions?scope={scope}")
    };
    page.totals = String::new();
    let send_on_save = c.sends && s.writer.as_ref().is_some_and(|w| w.settings().send_on_save);
    Ok(Html(
        ConfirmPage {
            page,
            c,
            send_on_save,
        }
        .render()?,
    )
    .into_response())
}

fn title_of(op: &Op) -> String {
    match op {
        Op::UseGlobal(k) => format!("{} · Use Global instead", views::short(*k)),
        Op::ToAgents(p) => format!("{p} · CLAUDE.md to AGENTS.md"),
        Op::ToGlobal(p, f) => format!("{p} · Move {f} to Global"),
        Op::FromGlobal(p) => format!("{p} · Copy Global here"),
        Op::Create(p, f) => format!("{p} · New {f}"),
    }
}

pub(crate) async fn run(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath((scope, file, action)): UrlPath<(String, String, String)>,
    Form(form): Form<ChangeForm>,
) -> Response {
    // Before any file is read or git runs for the plan.
    if let Err(e) = authorised(&s, &headers, &form.csrf) {
        return e.into_response(&headers);
    }
    let Some(op) = parse(&s.sources, &scope, &file, &action) else {
        return AppError::NotFound.into_response();
    };
    let creating = matches!(op, Op::Create(..));
    let mut p = match planned(&s, op, form.text.clone()).await {
        Ok(p) => p,
        Err(e) => return e.into_response(),
    };
    if !creating && p.confirm.base != form.base {
        return ChangeError::Refused(
            "The files changed since you opened this. Go back and look again; nothing changed."
                .to_owned(),
        )
        .into_response(&headers);
    }
    if form.claude.is_some() && !p.confirm.offer_claude.is_empty() {
        p.writes.push(Write::ClaudeAlongside);
        p.repos.push(RepoChange {
            kind: RepoKind::Knowledge,
            root: s.sources.knowledge.clone(),
            paths: vec![PathBuf::from(PLACEMENTS_FILE)],
            message: "Let Claude Code read AGENTS.md beside CLAUDE.md".to_owned(),
        });
    }
    let base = p.confirm.base.clone();
    let inputs = p.inputs.clone();
    let writes = p.writes.clone();
    let knowledge = s.sources.knowledge.clone();
    let apply: change::Apply = Box::new(move || {
        // Under the git lock: the files must still be what the confirmation showed.
        if fingerprint(&inputs)? != base {
            return Err("The files changed since you opened this.".to_owned());
        }
        apply_writes(&writes, &knowledge)
    });
    let outcome = match change::commit_change(&s, &headers, &form.csrf, p.repos, apply).await {
        Ok(o) => o,
        Err(e) => return e.into_response(&headers),
    };
    let mut next = p.next;
    if let Some(r) = p.remove_after {
        // Git can't restore CLAUDE.local.md, so it goes only once Global has its text.
        let sources = Arc::clone(&s.sources);
        let removed = tokio::task::spawn_blocking(move || {
            let global = read_text(&sources.knowledge.join("AGENTS.md"))
                .ok()
                .flatten()
                .unwrap_or_default();
            let still = read_text(&r.path).ok().flatten();
            if still.as_deref() == Some(r.text.as_str()) && contained(&r.own, &global, |_| false) {
                fs::remove_file(&r.path).is_ok()
            } else {
                false
            }
        })
        .await
        .unwrap_or(false);
        // commit_change cleared the cache before the file went; the next page reads again.
        s.invalidate().await;
        if !removed {
            next.push_str(if next.contains('?') {
                "&kept=1"
            } else {
                "?kept=1"
            });
        }
    }
    if creating && matches!(outcome, Outcome::Unchanged) {
        // CLAUDE.local.md: made, but nothing to commit (it is kept out of git).
        return Redirect::to(&next).into_response();
    }
    change::respond(&s, &headers, &outcome, &next)
}

/// A change's session check, done before the change reads anything.
/// [`change::commit_change`] checks again under its own rules.
fn authorised(s: &AppState, headers: &HeaderMap, csrf: &str) -> Result<(), ChangeError> {
    let w = s.writer.as_ref().ok_or(ChangeError::ReadOnly)?;
    if w.authorised(headers, csrf) {
        Ok(())
    } else {
        Err(ChangeError::Forbidden)
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ClaudeForm {
    csrf: String,
    value: String,
}

/// Sets Claude Code's AGENTS.md option for every machine (`atlas/placements.toml`).
pub(crate) async fn claude_option(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ClaudeForm>,
) -> Response {
    if let Err(e) = authorised(&s, &headers, &form.csrf) {
        return e.into_response(&headers);
    }
    let Some(choice) = ClaudeAgentsMd::from_id(&form.value) else {
        return AppError::Bad("Unknown choice.".to_owned()).into_response();
    };
    let knowledge = s.sources.knowledge.clone();
    let repos = vec![RepoChange {
        kind: RepoKind::Knowledge,
        root: knowledge.clone(),
        paths: vec![PathBuf::from(PLACEMENTS_FILE)],
        message: format!("Claude Code reads AGENTS.md: {}", choice.id()),
    }];
    let apply: change::Apply = Box::new(move || set_claude(&knowledge, choice));
    match change::commit_change(&s, &headers, &form.csrf, repos, apply).await {
        Ok(outcome) => change::respond(&s, &headers, &outcome, "/instructions"),
        Err(e) => e.into_response(&headers),
    }
}

/// Names of the files in a project's top folder that agents read and Atlas lists:
/// [`PROJECT_FILES`] that exist, minus a `CLAUDE.md` that only points at `AGENTS.md`.
#[must_use]
pub fn listed(facts: &ProjectFacts) -> BTreeSet<&'static str> {
    PROJECT_FILES
        .iter()
        .copied()
        .filter(|f| facts.present.contains_key(*f))
        .filter(|f| !(*f == "CLAUDE.md" && facts.claude_points_at_agents))
        .collect()
}

/// The words for how an agent reaches a project file.
#[must_use]
pub fn project_words(access: &Access, name: &str) -> String {
    match access {
        Access::Direct => "reads it".to_owned(),
        Access::Link(f) => format!("reads it through {f}, a link to it"),
        Access::Import(f) => format!("reads it through {f}'s import"),
        Access::Setting => "reads it through its AGENTS.md option".to_owned(),
        Access::None(why) => why_words(why, name),
    }
}

#[must_use]
pub fn why_words(why: &Why, name: &str) -> String {
    match why {
        Why::NotItsName => format!("doesn't read {name}"),
        Why::Earlier(e) => format!("reads {e} instead"),
        Why::SettingOff => "skips it: its AGENTS.md option is off".to_owned(),
        Why::ClaudeFiles(f) => format!("skips it while {f} is there (its AGENTS.md option)"),
        Why::Ignored => "skips files git ignores".to_owned(),
        Why::NoFiles => "reads no such file".to_owned(),
    }
}

/// The words for how an agent reaches Global on this machine.
#[must_use]
pub fn home_words(access: &Access, agent: &AgentHome) -> String {
    match access {
        Access::Link(f) => format!("reads it through ~/{f}, a link to it"),
        Access::Import(f) => format!("reads it through ~/{f}'s import"),
        Access::Direct | Access::Setting => "reads it".to_owned(),
        Access::None(Why::Earlier(e)) => format!("reads ~/{e} instead"),
        Access::None(_) if agent.own_text.is_some() => format!("uses its own {}", agent.shown),
        Access::None(_) => format!("no {} yet; atlasd links it", agent.shown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_cover_only_the_project_asked_for() {
        let tmp = tempfile::tempdir().unwrap();
        let project = |name: &str| {
            let root = tmp.path().join(name);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("AGENTS.md"), "# Rules\n").unwrap();
            atlas_fs::Project {
                name: Slug::new(name).unwrap(),
                root,
            }
        };
        let sources = Sources {
            home: tmp.path().to_path_buf(),
            knowledge: tmp.path().join("k"),
            public_skills: None,
            projects: vec![project("shop"), project("blog")],
        };
        assert!(read_facts(&sources, None, None).projects.is_empty());
        let shop = Slug::new("shop").unwrap();
        let facts = read_facts(&sources, None, Some(&shop));
        assert_eq!(facts.projects.keys().collect::<Vec<_>>(), [&shop]);
        assert!(facts.projects[&shop].present.contains_key("AGENTS.md"));
    }

    #[test]
    fn only_known_actions_parse() {
        let tmp = tempfile::tempdir().unwrap();
        let sources = Sources {
            home: tmp.path().to_path_buf(),
            knowledge: tmp.path().join("k"),
            public_skills: None,
            projects: vec![atlas_fs::Project {
                name: Slug::new("shop").unwrap(),
                root: tmp.path().join("shop"),
            }],
        };
        let shop = Slug::new("shop").unwrap();
        assert_eq!(
            parse(&sources, "global", "codex", "use-global"),
            Some(Op::UseGlobal(HarnessKind::Codex))
        );
        assert_eq!(parse(&sources, "global", "t3", "use-global"), None);
        assert_eq!(
            parse(&sources, "shop", "CLAUDE.md", "to-agents"),
            Some(Op::ToAgents(shop.clone()))
        );
        assert_eq!(parse(&sources, "shop", "AGENTS.md", "to-agents"), None);
        assert_eq!(parse(&sources, "shop", "README.md", "to-global"), None);
        assert_eq!(parse(&sources, "shop", "../x", "create"), None);
        assert_eq!(parse(&sources, "elsewhere", "AGENTS.md", "create"), None);
        assert_eq!(
            parse(&sources, "shop", "CLAUDE.local.md", "create"),
            Some(Op::Create(shop, "CLAUDE.local.md"))
        );
    }
}
