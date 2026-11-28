//! **Check for updates** for skills installed from GitHub with the `skills` CLI
//! (`npx skills`), adapted from the owner's T3 Code. The skill's folder is in a store repo
//! Atlas commits; its lock record (`atlas_fs::lock`) names the GitHub source and the
//! version installed (the baseline).
//!
//! - **Check** (a button, never automatic) lists each source repository's tree once, at
//!   most [`MAX_REPOS`] of them, four at a time, without signing in. The listings are
//!   kept in memory for the hub's run; nothing is written.
//! - **Preview** shows what updating does to each file, as a diff, before anything is
//!   written. With the baseline a difference is the source's change, yours, or both: only
//!   theirs takes theirs, only yours keeps yours, both is merged line by line, and clashing
//!   lines leave the owner to choose per file (keep mine or take theirs). A global record's
//!   baseline is a git tree SHA, fetched again from GitHub; a project record's is only a
//!   hash, so when the owner edited such a skill every difference is theirs to choose.
//! - **Apply** writes the folder and the lock record as one change (`change::commit_change`),
//!   leaving the lock as `npx skills update` would, so the CLI then sees the skill as
//!   current. A lock outside every repo Atlas commits (the global one usually is) is left
//!   alone, and the confirmation says so.
//!
//! Fetched content is never executed; paths from GitHub are checked before they are joined
//! to a folder; everything shown is escaped by the templates.

mod github;
mod plan;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use askama::Template;
use atlas_core::catalog::SkillHome;
use atlas_core::catalog::edit::RepoKind;
use atlas_core::kernel::{Scope, Slug};
use atlas_fs::Sources;
use atlas_fs::library::{MAX_BYTES, MAX_FILES, Tree, TreeFile};
use atlas_fs::lock::{self, Baseline, GitHubSource, LockFile, LockKind, TreeItem};
use axum::Form;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::Deserialize;

use crate::change::{self, RepoChange};
use crate::views::Page;
use crate::{AppError, AppState};
use github::{GitHub, Problem, RepoTree};
use plan::{Change, Outcome, Side, Upstream};

/// Source repositories one check lists; the rest wait for the next.
pub(crate) const MAX_REPOS: usize = 20;
const CONCURRENCY: usize = 4;
/// Files a check may fetch to tell whether an edited project skill's source moved.
const CHECK_DOWNLOADS: usize = 5;
/// Diff text shown on one preview, in all.
const SHOWN_TOTAL: usize = 1024 * 1024;

/// The hub's update checker: the GitHub reader and the last check's listings.
#[derive(Debug)]
pub(crate) struct Updates {
    github: GitHub,
    global_lock: PathBuf,
    last: tokio::sync::Mutex<Option<Arc<Check>>>,
}

/// One check: each repository's listing (or why it couldn't be read).
#[derive(Debug)]
struct Check {
    at: SystemTime,
    repos: HashMap<String, Result<Arc<RepoTree>, Problem>>,
    /// Repositories over [`MAX_REPOS`], not listed.
    left_out: usize,
}

impl Updates {
    pub(crate) fn new(sources: &Sources) -> Self {
        let xdg = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
        Self::with(
            github::API,
            lock::global_lock_path(&sources.home, xdg.as_deref()),
        )
    }

    pub(crate) fn with(api: &str, global_lock: PathBuf) -> Self {
        Self {
            github: GitHub::new(api),
            global_lock,
            last: tokio::sync::Mutex::new(None),
        }
    }
}

// ---------------------------------------------------------------- locked skills

/// Where a lock file is: inside a repo Atlas commits, or elsewhere.
#[derive(Clone, Debug)]
enum LockPlace {
    Repo {
        kind: RepoKind,
        root: PathBuf,
        rel: String,
    },
    Outside {
        shown: String,
    },
}

/// A skill in the store with a GitHub lock record.
#[derive(Clone, Debug)]
struct Locked {
    home: SkillHome,
    name: Slug,
    kind: RepoKind,
    root: PathBuf,
    /// The skill's folder relative to `root`.
    rel: String,
    src: GitHubSource,
    /// `source` as the record has it.
    source: String,
    baseline: Option<Baseline>,
    lock: Arc<LockFile>,
    place: LockPlace,
}

impl Locked {
    fn folder(&self) -> PathBuf {
        self.root.join(&self.rel)
    }

    fn href(&self) -> String {
        format!("/skills/{}/{}/update", self.home.key(), self.name)
    }
}

fn repo_key(src: &GitHubSource) -> String {
    format!(
        "{}/{}@{}",
        src.owner.to_ascii_lowercase(),
        src.repo.to_ascii_lowercase(),
        src.git_ref.as_deref().unwrap_or("")
    )
}

fn store_roots(sources: &Sources) -> Vec<(RepoKind, PathBuf)> {
    let mut roots = vec![(RepoKind::Knowledge, sources.knowledge.clone())];
    if let Some(p) = &sources.public_skills {
        roots.push((RepoKind::PublicSkills, p.clone()));
    }
    for p in &sources.projects {
        roots.push((RepoKind::Project(p.name.clone()), p.root.clone()));
    }
    roots
}

fn place_of(path: &Path, roots: &[(RepoKind, PathBuf)], sources: &Sources) -> LockPlace {
    let outside = || LockPlace::Outside {
        shown: sources.display(path),
    };
    let Ok(real) = std::fs::canonicalize(path) else {
        return outside();
    };
    for (kind, root) in roots {
        let Ok(root_real) = std::fs::canonicalize(root) else {
            continue;
        };
        if let Ok(rel) = real.strip_prefix(&root_real)
            && let Some(rel) = rel.to_str().filter(|r| lock::is_safe_rel(r))
        {
            return LockPlace::Repo {
                kind: kind.clone(),
                root: root.clone(),
                rel: rel.to_owned(),
            };
        }
    }
    outside()
}

/// Every store skill the `skills` CLI recorded as installed from GitHub: Global ones
/// (knowledge and public) from the global lock, a project's from its own lock.
fn locate_all(
    sources: &Sources,
    skills: &[atlas_core::catalog::Skill],
    global_lock: &Path,
) -> Vec<Locked> {
    let roots = store_roots(sources);
    let read = |path: &Path, kind| match lock::read_lock(path, kind) {
        Ok(found) => found.map(Arc::new),
        Err(e) => {
            tracing::info!("{} not read: {e}", sources.display(path));
            None
        }
    };
    let global = read(global_lock, LockKind::Global);
    let mut projects: HashMap<Slug, Option<Arc<LockFile>>> = HashMap::new();
    let mut out = Vec::new();
    for skill in skills {
        let name = &skill.name;
        let (kind, root, rel, file) = match &skill.home {
            SkillHome::Private => (
                RepoKind::Knowledge,
                sources.knowledge.clone(),
                format!("skills/{name}"),
                global.clone(),
            ),
            SkillHome::Public => {
                let Some(root) = sources.public_skills.clone() else {
                    continue;
                };
                (
                    RepoKind::PublicSkills,
                    root,
                    name.to_string(),
                    global.clone(),
                )
            }
            SkillHome::Project(p) => {
                let Some(project) = sources.projects.iter().find(|x| &x.name == p) else {
                    continue;
                };
                let file = projects
                    .entry(p.clone())
                    .or_insert_with(|| {
                        read(&lock::project_lock_path(&project.root), LockKind::Project)
                    })
                    .clone();
                (
                    RepoKind::Project(p.clone()),
                    project.root.clone(),
                    format!(".agents/skills/{name}"),
                    file,
                )
            }
        };
        let Some(file) = file else { continue };
        let Some(record) = file.records.get(name.as_str()) else {
            continue;
        };
        let Some(src) = record.github() else { continue };
        out.push(Locked {
            home: skill.home.clone(),
            name: name.clone(),
            kind,
            root,
            rel,
            baseline: record.baseline(file.kind),
            source: record.source.clone(),
            src,
            place: place_of(&file.path, &roots, sources),
            lock: file,
        });
    }
    out
}

// ---------------------------------------------------------------- the local copy

#[derive(Clone, Debug)]
struct LocalFile {
    bytes: Arc<[u8]>,
    executable: bool,
    sha: String,
}

/// A skill's folder as it is now.
#[derive(Clone, Debug)]
struct Local {
    files: BTreeMap<String, LocalFile>,
    side: Side,
    /// The compared files, with modes.
    items: Vec<TreeItem>,
    /// The compared files' tree SHA, which pins the preview to what Apply writes over.
    pin: String,
    /// Every file's tree SHA: the recorded `skillFolderHash` for an unedited copy.
    raw_sha: String,
    content_hash: String,
}

impl Local {
    fn by_sha(&self, sha: &str) -> Option<Arc<[u8]>> {
        self.files
            .values()
            .find(|f| f.sha == sha)
            .map(|f| Arc::clone(&f.bytes))
    }
}

fn read_local(folder: &Path) -> Result<Local, String> {
    let tree = atlas_fs::library::read_tree(folder).map_err(|e| format!("{e}."))?;
    if !tree.skipped.is_empty() {
        return Err("It has links or a .git folder inside, which Atlas doesn't update.".to_owned());
    }
    let mut files = BTreeMap::new();
    for f in tree.files {
        let rel = f
            .rel
            .to_str()
            .filter(|r| lock::is_safe_rel(r))
            .ok_or_else(|| "It has a file name Atlas can't read.".to_owned())?
            .to_owned();
        let executable = f.executable;
        files.insert(
            rel,
            LocalFile {
                sha: lock::git_blob_sha(&f.bytes),
                bytes: f.bytes.into(),
                executable,
            },
        );
    }
    let item = |(p, f): (&String, &LocalFile)| TreeItem {
        path: p.clone(),
        mode: if f.executable { "100755" } else { "100644" }.to_owned(),
        sha: f.sha.clone(),
    };
    let all: Vec<TreeItem> = files.iter().map(item).collect();
    let items: Vec<TreeItem> = all
        .iter()
        .filter(|i| !lock::is_ignored_path(&i.path))
        .cloned()
        .collect();
    let hashed: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(p, f)| (p.as_str(), &f.bytes[..]))
        .collect();
    Ok(Local {
        side: items
            .iter()
            .map(|i| (i.path.clone(), i.sha.clone()))
            .collect(),
        pin: lock::git_tree_sha(&items),
        raw_sha: lock::git_tree_sha(&all),
        content_hash: lock::computed_hash(&hashed),
        items,
        files,
    })
}

// ---------------------------------------------------------------- comparing

/// Where a skill stands against its source.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Standing {
    Current,
    /// The source moved on. `edited`: the owner changed the local copy too.
    Update {
        edited: bool,
    },
    /// Only the owner changed it; the source has nothing new.
    EditedOnly,
    /// It differs, and the record has no installed version to tell whose change it is.
    Differs,
    /// The record's folder is no longer in the source.
    Removed,
    NotChecked,
    Unknown(String),
}

impl Standing {
    fn words(&self) -> String {
        match self {
            Self::Current => "Current".to_owned(),
            Self::Update { edited: false } => "Update available".to_owned(),
            Self::Update { edited: true } => "Update available · you edited it".to_owned(),
            Self::EditedOnly => "Current, with your edits".to_owned(),
            Self::Differs => "Differs from its source".to_owned(),
            Self::Removed => "No longer in its source".to_owned(),
            Self::NotChecked => "Not checked yet".to_owned(),
            Self::Unknown(why) => why.clone(),
        }
    }

    const fn updatable(&self) -> bool {
        matches!(self, Self::Update { .. } | Self::Differs)
    }
}

impl Updates {
    async fn bytes_of(&self, l: &Locked, local: &Local, sha: &str) -> Result<Arc<[u8]>, Problem> {
        if let Some(b) = local.by_sha(sha).or_else(|| self.github.cached(sha)) {
            return Ok(b);
        }
        self.github.blob(&l.src.owner, &l.src.repo, sha).await
    }

    /// The source's folder for a record, from its repository's listing.
    async fn upstream(&self, l: &Locked, tree: &RepoTree) -> Result<Option<Upstream>, Problem> {
        let Some((folder, sha)) = plan::find_folder(tree, &l.src, l.name.as_str()) else {
            return if tree.truncated {
                Err(Problem::TooBig)
            } else {
                Ok(None)
            };
        };
        // A cut-off listing may miss some of the folder's files; its own listing has them.
        if tree.truncated {
            let own = self
                .github
                .tree_by_sha(&l.src.owner, &l.src.repo, &sha)
                .await?;
            return Ok(own.map(|t| plan::upstream_of(&t.tree, "", &sha)));
        }
        Ok(Some(plan::upstream_of(&tree.tree, &folder, &sha)))
    }

    /// The CLI's `computedHash` of the source's files: as it hashes a download (with the
    /// files an install leaves out) and as it hashes an install (without them). Files the
    /// local copy has are read from it; at most `budget` others are fetched.
    async fn upstream_hashes(
        &self,
        l: &Locked,
        up: &Upstream,
        local: &Local,
        budget: Option<usize>,
    ) -> Result<Option<[String; 2]>, Problem> {
        let missing = up
            .files
            .values()
            .filter(|f| local.by_sha(&f.sha).is_none() && self.github.cached(&f.sha).is_none())
            .count();
        if budget.is_some_and(|b| missing > b) || up.files.len() > MAX_FILES {
            return Ok(None);
        }
        let mut files = Vec::with_capacity(up.files.len());
        let mut total = 0_u64;
        for (path, f) in &up.files {
            let bytes = self.bytes_of(l, local, &f.sha).await?;
            total += bytes.len() as u64;
            if total > MAX_BYTES {
                return Err(Problem::TooBig);
            }
            files.push((path.clone(), bytes));
        }
        let all: Vec<(&str, &[u8])> = files.iter().map(|(p, b)| (p.as_str(), &b[..])).collect();
        let installed: Vec<(&str, &[u8])> = all
            .iter()
            .copied()
            .filter(|(p, _)| !lock::is_ignored_path(p))
            .collect();
        Ok(Some([
            lock::computed_hash(&all),
            lock::computed_hash(&installed),
        ]))
    }

    /// Tells the source's changes from the owner's, and the version both started from when
    /// it is known. `full` allows the downloads a preview needs; a check makes at most a
    /// few for an edited project skill.
    async fn compare(
        &self,
        l: &Locked,
        local: &Local,
        up: &Upstream,
        full: bool,
    ) -> Result<(Standing, Option<Side>), Problem> {
        if local.side == up.side {
            return Ok((Standing::Current, Some(up.side.clone())));
        }
        match &l.baseline {
            None => Ok((Standing::Differs, None)),
            Some(Baseline::Tree(base)) => {
                if &up.sha == base {
                    return Ok((Standing::EditedOnly, Some(up.side.clone())));
                }
                // An install lacks the files the CLI leaves out; with the source's own
                // copies of them added back an unedited copy hashes to the recorded tree.
                let mut with_left_out = local.items.clone();
                with_left_out.extend(
                    up.files
                        .iter()
                        .filter(|(p, _)| lock::is_ignored_path(p))
                        .map(|(p, f)| TreeItem {
                            path: p.clone(),
                            mode: if f.executable { "100755" } else { "100644" }.to_owned(),
                            sha: f.sha.clone(),
                        }),
                );
                if &local.raw_sha == base || &lock::git_tree_sha(&with_left_out) == base {
                    return Ok((Standing::Update { edited: false }, Some(local.side.clone())));
                }
                if !full {
                    return Ok((Standing::Update { edited: true }, None));
                }
                let found = self
                    .github
                    .tree_by_sha(&l.src.owner, &l.src.repo, base)
                    .await?;
                // GitHub no longer has the installed version: no file can be told apart.
                Ok(
                    found.map_or((Standing::Update { edited: true }, None), |t| {
                        let base_side = plan::upstream_of(&t.tree, "", base).side;
                        let edited = base_side != local.side;
                        (Standing::Update { edited }, Some(base_side))
                    }),
                )
            }
            Some(Baseline::Content(hash)) => {
                if &local.content_hash == hash {
                    return Ok((Standing::Update { edited: false }, Some(local.side.clone())));
                }
                let budget = (!full).then_some(CHECK_DOWNLOADS);
                match self.upstream_hashes(l, up, local, budget).await? {
                    None => Ok((Standing::Differs, None)),
                    Some(h) if h.contains(hash) => {
                        Ok((Standing::EditedOnly, Some(up.side.clone())))
                    }
                    Some(_) => Ok((Standing::Update { edited: true }, None)),
                }
            }
        }
    }

    /// The last check's listing of this skill's repository.
    async fn listing(&self, l: &Locked) -> Option<Result<Arc<RepoTree>, Problem>> {
        let last = self.last.lock().await.clone()?;
        last.repos.get(&repo_key(&l.src)).cloned()
    }

    /// Where one skill stands, from the last check's listing (no new listing request).
    async fn state_of(&self, l: &Locked, local: Result<Local, String>) -> Standing {
        let Some(listing) = self.listing(l).await else {
            return Standing::NotChecked;
        };
        let local = match local {
            Ok(local) => local,
            Err(why) => return Standing::Unknown(why),
        };
        let tree = match listing {
            Ok(tree) => tree,
            Err(p) => return Standing::Unknown(p.sentence()),
        };
        match self.upstream(l, &tree).await {
            Err(p) => Standing::Unknown(p.sentence()),
            Ok(None) => Standing::Removed,
            Ok(Some(up)) if up.unsupported => Standing::Unknown(
                "Its source has links, submodules or file names Atlas doesn't update.".to_owned(),
            ),
            Ok(Some(up)) => match self.compare(l, &local, &up, false).await {
                Ok((state, _)) => state,
                Err(p) => Standing::Unknown(p.sentence()),
            },
        }
    }
}

// ---------------------------------------------------------------- the plan

/// What updating does to one file.
#[derive(Clone, Debug)]
enum Fate {
    Theirs,
    Mine,
    Merged(Vec<u8>),
    Clash,
}

#[derive(Clone, Debug)]
struct Prep {
    path: String,
    change: Change,
    fate: Fate,
    mine: Option<Arc<[u8]>>,
    theirs: Option<Arc<[u8]>>,
    theirs_executable: bool,
}

/// A skill's update worked out, file by file. Nothing is written.
#[derive(Debug)]
struct Prepared {
    state: Standing,
    local: Local,
    up: Upstream,
    files: Vec<Prep>,
}

impl Updates {
    async fn prepare(&self, l: &Locked, tree: &RepoTree) -> Result<Prepared, String> {
        let folder = l.folder();
        let local = tokio::task::spawn_blocking(move || read_local(&folder))
            .await
            .map_err(|_| "Reading the skill stopped unexpectedly.".to_owned())??;
        let sentence = |p: Problem| p.sentence();
        let up = self
            .upstream(l, tree)
            .await
            .map_err(sentence)?
            .ok_or_else(|| "The skill is no longer in its source.".to_owned())?;
        if up.unsupported {
            return Err(
                "Its source has links, submodules or file names Atlas doesn't update.".to_owned(),
            );
        }
        if up.files.len() > MAX_FILES {
            return Err("Its source is too big to update.".to_owned());
        }
        let (state, base) = self.compare(l, &local, &up, true).await.map_err(sentence)?;
        let mut files = Vec::new();
        if state.updatable() {
            let mut total = 0_u64;
            for p in plan::plan_files(&local.side, &up.side, base.as_ref()) {
                let mine = local.files.get(&p.path).map(|f| Arc::clone(&f.bytes));
                let theirs_file = up
                    .files
                    .get(&p.path)
                    .filter(|_| up.side.contains_key(&p.path));
                let theirs = match theirs_file {
                    Some(f) => Some(self.bytes_of(l, &local, &f.sha).await.map_err(sentence)?),
                    None => None,
                };
                total += theirs.as_ref().map_or(0, |b| b.len() as u64);
                if total > MAX_BYTES {
                    return Err("The update is too big (over 20 MB).".to_owned());
                }
                let fate = match p.outcome {
                    Outcome::Theirs => Fate::Theirs,
                    Outcome::Mine => Fate::Mine,
                    Outcome::Clash => Fate::Clash,
                    Outcome::Merge => {
                        let base_sha = base.as_ref().and_then(|b| b.get(&p.path));
                        match (base_sha, &mine, &theirs) {
                            (Some(sha), Some(m), Some(t)) => {
                                let b = self.bytes_of(l, &local, sha).await.map_err(sentence)?;
                                plan::merge3(&b, m, t).map_or(Fate::Clash, Fate::Merged)
                            }
                            _ => Fate::Clash,
                        }
                    }
                };
                files.push(Prep {
                    path: p.path,
                    change: p.change,
                    fate,
                    mine,
                    theirs,
                    theirs_executable: theirs_file.is_some_and(|f| f.executable),
                });
            }
        }
        Ok(Prepared {
            state,
            local,
            up,
            files,
        })
    }
}

// ---------------------------------------------------------------- pages

#[derive(Debug)]
struct Row {
    name: String,
    source: String,
    state: String,
    href: Option<String>,
    problem: bool,
}

#[derive(Template)]
#[template(path = "updates.html")]
struct UpdatesPage {
    page: Page,
    rows: Vec<Row>,
    checked: String,
    left_out: usize,
}

#[derive(Debug)]
struct FileView {
    n: usize,
    path: String,
    change: &'static str,
    fate: &'static str,
    clash: bool,
    lines: Vec<plan::DiffLine>,
    note: &'static str,
}

#[derive(Template)]
#[template(path = "update.html")]
struct UpdatePage {
    page: Page,
    action: String,
    source: String,
    state: String,
    lock_note: String,
    files: Vec<FileView>,
    local_pin: String,
    upstream_pin: String,
    can_apply: bool,
    checked: bool,
}

/// Reading these pages fetches from GitHub, so they need a passkey session: its cookie is
/// `SameSite=Strict`, so another site can't make the hub spend the request limit.
fn require_session(s: &AppState, headers: &HeaderMap, next: &str) -> Option<Response> {
    let Some(writer) = s.writer.as_ref() else {
        return Some(AppError::NotFound.into_response());
    };
    writer
        .auth
        .session(headers)
        .is_none()
        .then(|| Redirect::to(&format!("/signin?next={next}")).into_response())
}

async fn locate(s: &AppState) -> Result<Vec<Locked>, AppError> {
    let snap = s.snapshot().await?;
    let sources = Arc::clone(&s.sources);
    let global = s.updates.global_lock.clone();
    tokio::task::spawn_blocking(move || locate_all(&sources, &snap.catalog.skills, &global))
        .await
        .map_err(|_| AppError::Internal)
}

/// Runs `jobs` at most [`CONCURRENCY`] at a time and returns their results in order.
async fn bounded<T: Send + 'static>(
    jobs: Vec<std::pin::Pin<Box<dyn Future<Output = T> + Send>>>,
) -> Vec<T> {
    let gate = Arc::new(tokio::sync::Semaphore::new(CONCURRENCY));
    let mut set = tokio::task::JoinSet::new();
    for (i, job) in jobs.into_iter().enumerate() {
        let gate = Arc::clone(&gate);
        set.spawn(async move {
            let _permit = gate.acquire_owned().await;
            (i, job.await)
        });
    }
    let mut out: Vec<(usize, T)> = Vec::new();
    while let Some(done) = set.join_next().await {
        if let Ok(item) = done {
            out.push(item);
        }
    }
    out.sort_by_key(|(i, _)| *i);
    out.into_iter().map(|(_, t)| t).collect()
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UpdatesQuery {
    #[serde(default)]
    checked: Option<String>,
}

/// `GET /skills/updates`: each locked skill and where it stands since the last check.
pub(crate) async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<UpdatesQuery>,
) -> Result<Response, AppError> {
    if let Some(r) = require_session(&s, &headers, "/skills/updates") {
        return Ok(r);
    }
    let locked = locate(&s).await?;
    let jobs = locked
        .iter()
        .map(|l| {
            let (s, l) = (s.clone(), l.clone());
            Box::pin(async move {
                let folder = l.folder();
                let local = tokio::task::spawn_blocking(move || read_local(&folder))
                    .await
                    .unwrap_or_else(|_| Err("Reading the skill stopped.".to_owned()));
                s.updates.state_of(&l, local).await
            }) as std::pin::Pin<Box<dyn Future<Output = Standing> + Send>>
        })
        .collect();
    let states = bounded(jobs).await;
    let rows = locked
        .iter()
        .zip(states)
        .map(|(l, state)| Row {
            name: l.name.to_string(),
            source: l.src.slug(),
            href: state.updatable().then(|| l.href()),
            problem: matches!(state, Standing::Unknown(_) | Standing::Removed),
            state: state.words(),
        })
        .collect::<Vec<_>>();
    let last = s.updates.last.lock().await.clone();
    let snap = s.snapshot().await?;
    let mut page = s.page(&headers, "/skills", "Updates", &snap, &Scope::Global, false);
    page.totals = format!(
        "{} from GitHub · {} to update",
        rows.len(),
        rows.iter().filter(|r| r.href.is_some()).count()
    );
    if q.checked.is_some() {
        let problems: Vec<String> = last
            .iter()
            .flat_map(|c| c.repos.values())
            .filter_map(|r| r.as_ref().err().map(Problem::sentence))
            .collect();
        page.flash = problems
            .iter()
            .find(|p| p.contains("limit"))
            .cloned()
            .unwrap_or_else(|| "Checked.".to_owned());
    }
    Ok(Html(
        UpdatesPage {
            page,
            rows,
            checked: last
                .as_ref()
                .map_or_else(String::new, |c| github::clock(c.at)),
            left_out: last.as_ref().map_or(0, |c| c.left_out),
        }
        .render()?,
    )
    .into_response())
}

#[derive(Debug, Deserialize)]
pub(crate) struct CheckForm {
    #[serde(default)]
    csrf: String,
}

/// `POST /skills/updates/check`: lists each source repository once.
pub(crate) async fn check(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<CheckForm>,
) -> Result<Response, AppError> {
    let w = crate::edit::writer(&s)?;
    if !w.authorised(&headers, &form.csrf) {
        return Ok(crate::edit::forbidden(
            "Your session ended. Sign in again; nothing was checked.\n",
        ));
    }
    let locked = locate(&s).await?;
    let mut repos: BTreeMap<String, GitHubSource> = BTreeMap::new();
    for l in &locked {
        repos
            .entry(repo_key(&l.src))
            .or_insert_with(|| l.src.clone());
    }
    let left_out = repos.len().saturating_sub(MAX_REPOS);
    let picked: Vec<(String, GitHubSource)> = repos.into_iter().take(MAX_REPOS).collect();
    let jobs = picked
        .iter()
        .map(|(_, src)| {
            let (s, src) = (s.clone(), src.clone());
            Box::pin(async move {
                s.updates
                    .github
                    .repo_tree(&src.owner, &src.repo, src.git_ref.as_deref())
                    .await
                    .map(Arc::new)
            })
                as std::pin::Pin<Box<dyn Future<Output = Result<Arc<RepoTree>, Problem>> + Send>>
        })
        .collect();
    let trees = bounded(jobs).await;
    let check = Check {
        at: SystemTime::now(),
        repos: picked.into_iter().map(|(k, _)| k).zip(trees).collect(),
        left_out,
    };
    *s.updates.last.lock().await = Some(Arc::new(check));
    Ok(Redirect::to("/skills/updates?checked=1").into_response())
}

async fn find(s: &AppState, home: &str, name: &str) -> Result<Locked, AppError> {
    locate(s)
        .await?
        .into_iter()
        .find(|l| l.home.key() == home && l.name.as_str() == name)
        .ok_or(AppError::NotFound)
}

fn lock_note(l: &Locked) -> String {
    match &l.place {
        LockPlace::Repo { .. } if !l.lock.writable() => format!(
            "The skills CLI's lock has version {}, which Atlas doesn't write, so it stays as it is.",
            l.lock.version
        ),
        LockPlace::Repo { rel, .. } => {
            format!(
                "The skills CLI's lock ({rel}) is updated in the same change, so npx skills update sees it as current."
            )
        }
        LockPlace::Outside { shown } => format!(
            "The skills CLI's lock ({shown}) isn't in a repo Atlas commits, so Atlas leaves it alone; npx skills update may still offer this update."
        ),
    }
}

const fn change_word(c: Change) -> &'static str {
    match c {
        Change::Added => "New file",
        Change::Modified => "Changed",
        Change::Removed => "Removed upstream",
    }
}

fn views(files: &[Prep]) -> Vec<FileView> {
    let mut budget = SHOWN_TOTAL;
    files
        .iter()
        .enumerate()
        .map(|(n, f)| {
            let (fate, new): (&'static str, Option<&[u8]>) = match &f.fate {
                Fate::Theirs => ("Takes theirs", f.theirs.as_deref()),
                Fate::Merged(m) => ("Merged with your edits", Some(m)),
                Fate::Mine => ("Keeps yours", None),
                Fate::Clash => ("Clash: choose", f.theirs.as_deref()),
            };
            let shown = if matches!(f.fate, Fate::Mine) {
                None
            } else {
                plan::unified(f.mine.as_deref(), new)
            };
            let (lines, note) = match shown {
                _ if matches!(f.fate, Fate::Mine) => (Vec::new(), "Only you changed it."),
                None => (Vec::new(), "Binary or too large to show."),
                Some(lines) => {
                    let size: usize = lines.iter().map(|l| l.text.len() + 1).sum();
                    if size > budget {
                        (Vec::new(), "Too long to show here.")
                    } else {
                        budget -= size;
                        (lines, "")
                    }
                }
            };
            FileView {
                n,
                path: f.path.clone(),
                change: change_word(f.change),
                fate,
                clash: matches!(f.fate, Fate::Clash),
                lines,
                note,
            }
        })
        .collect()
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct DoneQuery {
    #[serde(default)]
    changed: Option<String>,
    #[serde(default)]
    unchanged: Option<String>,
    #[serde(default)]
    sent: Option<String>,
}

/// `GET /skills/{home}/{name}/update`: the preview.
pub(crate) async fn preview(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath((home, name)): UrlPath<(String, String)>,
    Query(q): Query<DoneQuery>,
) -> Result<Response, AppError> {
    let here = format!("/skills/{home}/{name}/update");
    if let Some(r) = require_session(&s, &headers, &here) {
        return Ok(r);
    }
    let l = find(&s, &home, &name).await?;
    let snap = s.snapshot().await?;
    let mut page = s.page(
        &headers,
        "/skills",
        format!("Update {name}"),
        &snap,
        &l.home.scope(),
        false,
    );
    if q.changed.is_some() || q.sent.is_some() {
        "Updated.".clone_into(&mut page.flash);
    } else if q.unchanged.is_some() {
        "Nothing changed, so nothing was committed.".clone_into(&mut page.flash);
    }
    let mut view = UpdatePage {
        page,
        action: here,
        source: l.src.slug(),
        state: Standing::NotChecked.words(),
        lock_note: lock_note(&l),
        files: Vec::new(),
        local_pin: String::new(),
        upstream_pin: String::new(),
        can_apply: false,
        checked: false,
    };
    match s.updates.listing(&l).await {
        None => {}
        Some(Err(p)) => {
            view.checked = true;
            view.state = p.sentence();
        }
        Some(Ok(tree)) => {
            view.checked = true;
            match s.updates.prepare(&l, &tree).await {
                Err(why) => view.state = why,
                Ok(p) => {
                    view.state = p.state.words();
                    view.can_apply = p.state.updatable();
                    view.files = views(&p.files);
                    view.local_pin = p.local.pin;
                    view.upstream_pin = p.up.sha;
                }
            }
        }
    }
    Ok(Html(view.render()?).into_response())
}

fn refuse(headers: &HeaderMap, msg: &str) -> Response {
    change::ChangeError::Refused(msg.to_owned()).into_response(headers)
}

/// ISO 8601 in UTC with milliseconds, as the CLI writes `updatedAt`.
fn iso_now() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    format!(
        "{}T{:02}:{:02}:{:02}.000Z",
        crate::edit::day(secs),
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

/// `POST /skills/{home}/{name}/update`: writes the update the preview showed, as one
/// change, and records it in the lock when the lock is in a repo Atlas commits.
pub(crate) async fn apply(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath((home, name)): UrlPath<(String, String)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Result<Response, AppError> {
    let csrf = field(&fields, "csrf").to_owned();
    let writer = crate::edit::writer(&s)?;
    // Checked here as well as in the change, so a stranger can't spend GitHub's limit.
    if !writer.authorised(&headers, &csrf) {
        return Ok(change::ChangeError::Forbidden.into_response(&headers));
    }
    let skill = find(&s, &home, &name).await?;
    let (repos, write) = match plan_change(&s, &skill, &fields).await {
        Ok(planned) => planned,
        Err(why) => return Ok(refuse(&headers, &why)),
    };
    match change::commit_change(&s, &headers, &csrf, repos, write).await {
        Ok(outcome) => Ok(change::respond(&s, &headers, &outcome, &skill.href())),
        Err(e) => Ok(e.into_response(&headers)),
    }
}

/// The new folder's files: bytes and whether each is executable, by path.
type NewFiles = BTreeMap<String, (Arc<[u8]>, bool)>;

fn field<'a>(fields: &'a [(String, String)], key: &str) -> &'a str {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map_or("", |(_, v)| v.as_str())
}

/// The new folder: the local copy with each file's fate applied, the owner's choice
/// deciding each clash. Also says whether any file changes.
fn resolve(prepared: &Prepared, fields: &[(String, String)]) -> Result<(NewFiles, bool), String> {
    let mut files: NewFiles = prepared
        .local
        .files
        .iter()
        .map(|(k, f)| (k.clone(), (Arc::clone(&f.bytes), f.executable)))
        .collect();
    let mut touched = false;
    for (n, f) in prepared.files.iter().enumerate() {
        let take_theirs = match &f.fate {
            Fate::Mine => false,
            Fate::Theirs => true,
            Fate::Merged(bytes) => {
                let exec = files.get(&f.path).is_some_and(|(_, x)| *x);
                files.insert(f.path.clone(), (Arc::from(bytes.as_slice()), exec));
                touched = true;
                false
            }
            Fate::Clash => {
                let choice = field(fields, &format!("c{n}"));
                if choice.is_empty() {
                    return Err(format!(
                        "Choose keep mine or take theirs for {}; nothing changed.",
                        f.path
                    ));
                }
                if field(fields, &format!("p{n}")) != f.path {
                    return Err("The preview's files don't match the update now. Open it again; nothing changed.".to_owned());
                }
                match choice {
                    "mine" => false,
                    "theirs" => true,
                    _ => {
                        return Err(
                            "That choice isn't keep mine or take theirs; nothing changed."
                                .to_owned(),
                        );
                    }
                }
            }
        };
        if take_theirs {
            touched = true;
            match &f.theirs {
                Some(b) => {
                    files.insert(f.path.clone(), (Arc::clone(b), f.theirs_executable));
                }
                None => {
                    files.remove(&f.path);
                }
            }
        }
    }
    Ok((files, touched))
}

/// Works the update out again (so it is exactly what the preview pinned) and returns the
/// repos it commits and the write to run inside the change.
async fn plan_change(
    s: &AppState,
    skill: &Locked,
    fields: &[(String, String)],
) -> Result<(Vec<RepoChange>, change::Apply), String> {
    let Some(Ok(tree)) = s.updates.listing(skill).await else {
        return Err("Check for updates first; nothing changed.".to_owned());
    };
    let prepared = s
        .updates
        .prepare(skill, &tree)
        .await
        .map_err(|why| format!("{why} Nothing changed."))?;
    if !prepared.state.updatable() {
        return Err("There is nothing to update; nothing changed.".to_owned());
    }
    if prepared.local.pin != field(fields, "local") || prepared.up.sha != field(fields, "upstream")
    {
        return Err(
            "The skill or its source changed since the preview. Open it again; nothing changed."
                .to_owned(),
        );
    }
    let (files, touched) = resolve(&prepared, fields)?;
    // What the lock records: the version taken, hashed the way its kind of record is. A
    // project record gets the hash `npx skills update` compares, over every file it
    // downloads, so the CLI then sees the skill as current.
    let baseline = match skill.lock.kind {
        LockKind::Global => Baseline::Tree(prepared.up.sha.clone()),
        LockKind::Project => match s
            .updates
            .upstream_hashes(skill, &prepared.up, &prepared.local, None)
            .await
        {
            Ok(Some([download, _])) => Baseline::Content(download),
            Ok(None) => return Err("Its source is too big to update; nothing changed.".to_owned()),
            Err(e) => return Err(format!("{} Nothing changed.", e.sentence())),
        },
    };
    let lock_target = match &skill.place {
        LockPlace::Repo { kind, root, rel } if skill.lock.writable() => {
            Some((kind.clone(), root.clone(), rel.clone()))
        }
        _ => None,
    };
    let message = format!("Update {} from {}", skill.name, skill.src.slug());
    let mut repos = Vec::new();
    if touched {
        repos.push(RepoChange {
            kind: skill.kind.clone(),
            root: skill.root.clone(),
            paths: vec![PathBuf::from(&skill.rel)],
            message: message.clone(),
        });
    }
    if let Some((kind, root, rel)) = &lock_target {
        if let Some(r) = repos.iter_mut().find(|r| &r.root == root) {
            r.paths.push(PathBuf::from(rel));
        } else {
            repos.push(RepoChange {
                kind: kind.clone(),
                root: root.clone(),
                paths: vec![PathBuf::from(rel)],
                message,
            });
        }
    }
    if repos.is_empty() {
        return Err("Keeping your version changes no file, and the lock isn't in a repo Atlas commits; nothing changed.".to_owned());
    }
    let new_tree = Tree {
        files: files
            .iter()
            .map(|(rel, (bytes, executable))| TreeFile {
                rel: PathBuf::from(rel),
                bytes: bytes.to_vec(),
                executable: *executable,
            })
            .collect(),
        skipped: Vec::new(),
    };
    let (root, rel) = (skill.root.clone(), skill.rel.clone());
    let (lock_path, lock_kind) = (skill.lock.path.clone(), skill.lock.kind);
    let (name, source) = (skill.name.to_string(), skill.source.clone());
    let now = iso_now();
    let write: change::Apply = Box::new(move || {
        if touched {
            write_folder(&root, &rel, &new_tree)?;
        }
        if let Some((_, lock_root, lock_rel)) = lock_target {
            // Read again under the git lock, so a change made meanwhile isn't overwritten.
            let fresh = lock::read_lock(&lock_path, lock_kind)
                .ok()
                .flatten()
                .and_then(|f| f.with_baseline(&name, &source, &baseline, &now))
                .ok_or_else(|| "The skills CLI's lock changed or can't be written.".to_owned())?;
            atlas_fs::library::write_file(&lock_root, &lock_rel, fresh.as_bytes())
                .map_err(|e| format!("Writing the lock failed: {e}."))?;
        }
        Ok(())
    });
    Ok((repos, write))
}

/// Replaces the skill's folder with `tree` (execute bits kept). Runs inside the change,
/// which puts the folder back from git if anything here fails.
fn write_folder(root: &Path, rel: &str, tree: &Tree) -> Result<(), String> {
    atlas_fs::library::remove_tree(root, rel)
        .map_err(|e| format!("Removing the old copy failed: {e}."))?;
    atlas_fs::library::write_tree(root, rel, tree)
        .map_err(|e| format!("Writing the update failed: {e}."))
}
