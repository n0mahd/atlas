//! **Check for updates** for skills installed from GitHub with the `skills` CLI
//! (`npx skills`), adapted from the owner's T3 Code. The skill's folder is in a store repo
//! Atlas commits; its lock record (`atlas_fs::lock`) names the GitHub source and the
//! version installed (the baseline).
//!
//! - **Check** (a button, never automatic) lists each source repository's tree once, at
//!   most [`MAX_REPOS`] of them (never listed first, then the oldest), four at a time,
//!   without signing in, plus what a cut-off listing or an edited project skill needs. The
//!   listings merge with earlier checks' and are kept in memory for the hub's run; nothing
//!   is written, and the pages showing them never ask GitHub.
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

/// GitHub's API, which the hub reads skill sources from.
pub(crate) const GITHUB_API: &str = github::API;

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
use plan::{Change, Outcome, Side, UpFile, Upstream};

/// Source repositories one check lists: those never listed first, then the oldest
/// listings; the rest wait for the next check.
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

/// The checks so far, merged: each repository's latest listing (or why it couldn't be
/// read). Pages read only this; only a check asks GitHub for listings.
#[derive(Debug)]
struct Check {
    /// When the latest check ran.
    at: SystemTime,
    /// Counts checks, so the oldest listings are asked for again first.
    round: u64,
    repos: HashMap<String, Listed>,
}

/// One repository's listing, from the check numbered `round`.
#[derive(Clone, Debug)]
struct Listed {
    round: u64,
    tree: Result<Arc<RepoTree>, Problem>,
    /// For a cut-off listing: each locked skill's folder listed on its own (checked
    /// against its SHA) by that check, by the folder's tree SHA.
    folders: HashMap<String, Result<Option<Arc<RepoTree>>, Problem>>,
}

/// How many requests working out a skill's standing may make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Asks {
    /// None: a page shown from the last check.
    Nothing,
    /// A few file downloads, during a check, for an edited project skill.
    Few,
    /// Whatever a preview or the update needs.
    All,
}

/// Where a record's folder is in a check's listing.
#[derive(Debug)]
enum Found {
    Here(Upstream),
    /// The source no longer has it.
    Gone,
    /// A cut-off listing's folder the check didn't list on its own (a skill added since),
    /// with its tree SHA.
    Unlisted(String),
}

impl Updates {
    /// The `skills` CLI's global lock at its default place under the home. The hub binary
    /// passes the real place (it reads the environment) with
    /// [`crate::AppState::with_skills_lock`].
    pub(crate) fn new(sources: &Sources) -> Self {
        Self::with(github::API, lock::global_lock_path(&sources.home, None))
    }

    /// Where the `skills` CLI's global lock is read from (the skill library uses it too).
    pub(crate) fn global_lock(&self) -> &Path {
        &self.global_lock
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
            .map(|i| {
                let file = UpFile {
                    sha: i.sha.clone(),
                    executable: i.mode == "100755",
                };
                (i.path.clone(), file)
            })
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

    /// The source's folder for a record, from the check's listing of its repository, or
    /// (with `Asks::All`) from GitHub when the check didn't list it on its own.
    async fn upstream(&self, l: &Locked, listed: &Listed, asks: Asks) -> Result<Found, Problem> {
        match find_upstream(l, listed)? {
            Found::Unlisted(sha) if asks == Asks::All => {
                let own = self
                    .github
                    .tree_by_sha(&l.src.owner, &l.src.repo, &sha)
                    .await?;
                Ok(own.map_or(Found::Gone, |t| {
                    Found::Here(plan::upstream_of(&t.tree, "", &sha))
                }))
            }
            found => Ok(found),
        }
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
        // The CLI's hash leaves `.git` and `node_modules` out, so they are never fetched.
        let hashed: Vec<(&String, &UpFile)> = up
            .files
            .iter()
            .filter(|(p, _)| !lock::is_unhashed_path(p))
            .collect();
        let missing = hashed
            .iter()
            .filter(|(_, f)| local.by_sha(&f.sha).is_none() && self.github.cached(&f.sha).is_none())
            .count();
        if budget.is_some_and(|b| missing > b) || hashed.len() > MAX_FILES {
            return Ok(None);
        }
        let mut files = Vec::with_capacity(hashed.len());
        let mut total = 0_u64;
        for (path, f) in hashed {
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
    /// it is known. `Asks::All` allows the downloads a preview needs; a check makes at most
    /// a few for an edited project skill, and a page shown from the check none.
    async fn compare(
        &self,
        l: &Locked,
        local: &Local,
        up: &Upstream,
        asks: Asks,
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
                if asks != Asks::All {
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
                let budget = match asks {
                    Asks::Nothing => Some(0),
                    Asks::Few => Some(CHECK_DOWNLOADS),
                    Asks::All => None,
                };
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

    /// The checks' listing of this skill's repository.
    async fn listing(&self, l: &Locked) -> Option<Listed> {
        let last = self.last.lock().await.clone()?;
        last.repos.get(&repo_key(&l.src)).cloned()
    }

    /// Where one skill stands, from the checks' listings. A page asks for
    /// `Asks::Nothing`, so showing it never calls GitHub.
    async fn state_of(&self, l: &Locked, local: Result<Local, String>, asks: Asks) -> Standing {
        let Some(listed) = self.listing(l).await else {
            return Standing::NotChecked;
        };
        let local = match local {
            Ok(local) => local,
            Err(why) => return Standing::Unknown(why),
        };
        match self.upstream(l, &listed, asks).await {
            Err(p) => Standing::Unknown(p.sentence()),
            Ok(Found::Gone) => Standing::Removed,
            Ok(Found::Unlisted(_)) => Standing::NotChecked,
            Ok(Found::Here(up)) if up.unsupported => Standing::Unknown(
                "Its source has links, submodules or file names Atlas doesn't update.".to_owned(),
            ),
            Ok(Found::Here(up)) => match self.compare(l, &local, &up, asks).await {
                Ok((state, _)) => state,
                Err(p) => Standing::Unknown(p.sentence()),
            },
        }
    }
}

impl Updates {
    /// Lists one repository for a check: its whole tree once and, when GitHub cut the
    /// listing off, each of these skills' folders on its own, so pages never need to.
    async fn list_repo(&self, skills: &[Locked], round: u64) -> Listed {
        let src = &skills[0].src;
        let tree = self
            .github
            .repo_tree(&src.owner, &src.repo, src.git_ref.as_deref())
            .await
            .map(Arc::new);
        let mut folders = HashMap::new();
        if let Ok(tree) = &tree
            && tree.truncated
        {
            for l in skills {
                let Some((_, sha)) = plan::find_folder(tree, &l.src, l.name.as_str()) else {
                    continue;
                };
                if folders.contains_key(&sha) {
                    continue;
                }
                let own = self
                    .github
                    .tree_by_sha(&src.owner, &src.repo, &sha)
                    .await
                    .map(|t| t.map(Arc::new));
                folders.insert(sha, own);
            }
        }
        Listed {
            round,
            tree,
            folders,
        }
    }
}

/// Source repositories of these skills no check has listed yet.
fn unchecked(locked: &[Locked], last: Option<&Check>) -> usize {
    let keys: std::collections::BTreeSet<String> =
        locked.iter().map(|l| repo_key(&l.src)).collect();
    keys.iter()
        .filter(|k| last.is_none_or(|c| !c.repos.contains_key(*k)))
        .count()
}

/// The record's folder in a check's listing, never asking GitHub. In a whole listing the
/// folder's SHA is trusted only once its entries add up to it; a cut-off listing's folder
/// comes from the check's own listing of it, already checked against its SHA.
fn find_upstream(l: &Locked, listed: &Listed) -> Result<Found, Problem> {
    let tree = listed.tree.as_ref().map_err(Clone::clone)?;
    let Some((folder, sha)) = plan::find_folder(tree, &l.src, l.name.as_str()) else {
        return if tree.truncated {
            Err(Problem::TooBig)
        } else {
            Ok(Found::Gone)
        };
    };
    if tree.truncated {
        return match listed.folders.get(&sha) {
            None => Ok(Found::Unlisted(sha)),
            Some(Err(p)) => Err(p.clone()),
            Some(Ok(None)) => Ok(Found::Gone),
            Some(Ok(Some(own))) => Ok(Found::Here(plan::upstream_of(&own.tree, "", &sha))),
        };
    }
    if !plan::folder_sha_holds(&tree.tree, &folder, &sha) {
        return Err(Problem::Unavailable);
    }
    Ok(Found::Here(plan::upstream_of(&tree.tree, &folder, &sha)))
}

// ---------------------------------------------------------------- the plan

/// What updating does to one file.
#[derive(Clone, Debug)]
enum Fate {
    Theirs,
    Mine,
    /// The merged bytes, and whether the file is executable (theirs when they changed it).
    Merged(Vec<u8>, bool),
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
    async fn prepare(&self, l: &Locked, listed: &Listed) -> Result<Prepared, String> {
        let folder = l.folder();
        let local = tokio::task::spawn_blocking(move || read_local(&folder))
            .await
            .map_err(|_| "Reading the skill stopped unexpectedly.".to_owned())??;
        let sentence = |p: Problem| p.sentence();
        let Found::Here(up) = self
            .upstream(l, listed, Asks::All)
            .await
            .map_err(sentence)?
        else {
            return Err("The skill is no longer in its source.".to_owned());
        };
        if up.unsupported {
            return Err(
                "Its source has links, submodules or file names Atlas doesn't update.".to_owned(),
            );
        }
        if up.files.len() > MAX_FILES {
            return Err("Its source is too big to update.".to_owned());
        }
        let (state, base) = self
            .compare(l, &local, &up, Asks::All)
            .await
            .map_err(sentence)?;
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
                        let base_file = base.as_ref().and_then(|b| b.get(&p.path));
                        match (base_file, &mine, &theirs, theirs_file) {
                            (Some(bf), Some(m), Some(t), Some(tf)) => {
                                let b =
                                    self.bytes_of(l, &local, &bf.sha).await.map_err(sentence)?;
                                let mine_exec =
                                    local.files.get(&p.path).is_some_and(|f| f.executable);
                                let exec = if tf.executable == bf.executable {
                                    mine_exec
                                } else {
                                    tf.executable
                                };
                                plan::merge3(&b, m, t)
                                    .map_or(Fate::Clash, |merged| Fate::Merged(merged, exec))
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
    /// Updating commits to the knowledge repo and the owner sends on save.
    send_on_save: bool,
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
        .then(|| Redirect::to(&crate::auth::signin_href(next)).into_response())
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
                s.updates.state_of(&l, local, Asks::Nothing).await
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
            .flat_map(|c| c.repos.values().filter(|r| r.round == c.round))
            .filter_map(|r| r.tree.as_ref().err().map(Problem::sentence))
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
            left_out: unchecked(&locked, last.as_deref()),
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
    let mut repos: BTreeMap<String, Vec<Locked>> = BTreeMap::new();
    for l in &locked {
        repos.entry(repo_key(&l.src)).or_default().push(l.clone());
    }
    let prev = s.updates.last.lock().await.clone();
    let round = prev.as_ref().map_or(1, |c| c.round + 1);
    // Never listed (or not read last time) first, then the oldest listings.
    let since = |key: &String| {
        prev.as_ref()
            .and_then(|c| c.repos.get(key))
            .filter(|r| r.tree.is_ok())
            .map_or(0, |r| r.round)
    };
    let mut order: Vec<&String> = repos.keys().collect();
    order.sort_by_key(|k| (since(k), (*k).clone()));
    let picked: Vec<(String, Vec<Locked>)> = order
        .into_iter()
        .take(MAX_REPOS)
        .map(|k| (k.clone(), repos[k].clone()))
        .collect();
    let jobs = picked
        .iter()
        .map(|(_, skills)| {
            let (s, skills) = (s.clone(), skills.clone());
            Box::pin(async move { s.updates.list_repo(&skills, round).await })
                as std::pin::Pin<Box<dyn Future<Output = Listed> + Send>>
        })
        .collect();
    let listed = bounded(jobs).await;
    // Merged into the checks before, keeping only repositories still locked.
    let mut merged: HashMap<String, Listed> = prev
        .iter()
        .flat_map(|c| c.repos.iter())
        .filter(|(k, _)| repos.contains_key(*k))
        .map(|(k, r)| (k.clone(), r.clone()))
        .collect();
    merged.extend(picked.iter().map(|(k, _)| k.clone()).zip(listed));
    *s.updates.last.lock().await = Some(Arc::new(Check {
        at: SystemTime::now(),
        round,
        repos: merged,
    }));
    // An edited project skill's standing may need a few files; the check fetches them
    // now (they stay cached), so the page it leads to asks GitHub nothing.
    let edited_projects: Vec<Locked> = picked
        .into_iter()
        .flat_map(|(_, skills)| skills)
        .filter(|l| matches!(l.baseline, Some(Baseline::Content(_))))
        .collect();
    let jobs = edited_projects
        .into_iter()
        .map(|l| {
            let s = s.clone();
            Box::pin(async move {
                let folder = l.folder();
                let local = tokio::task::spawn_blocking(move || read_local(&folder))
                    .await
                    .unwrap_or_else(|_| Err("Reading the skill stopped.".to_owned()));
                if local
                    .as_ref()
                    .is_ok_and(|x| l.baseline != Some(Baseline::Content(x.content_hash.clone())))
                {
                    s.updates.state_of(&l, local, Asks::Few).await;
                }
            }) as std::pin::Pin<Box<dyn Future<Output = ()> + Send>>
        })
        .collect();
    bounded(jobs).await;
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
                Fate::Merged(m, _) => ("Merged with your edits", Some(m)),
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
                Some(lines) if lines.is_empty() => (Vec::new(), "Only its execute bit changes."),
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

/// `GET /skills/{home}/{name}/update`: the preview.
pub(crate) async fn preview(
    State(s): State<AppState>,
    headers: HeaderMap,
    UrlPath((home, name)): UrlPath<(String, String)>,
    Query(q): Query<crate::ListQuery>,
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
    // `apply` answers through `change::respond`, as every other change does.
    q.apply(&mut page);
    let sends = l.kind == RepoKind::Knowledge
        || matches!(
            &l.place,
            LockPlace::Repo {
                kind: RepoKind::Knowledge,
                ..
            }
        );
    let send_on_save = sends && s.writer.as_ref().is_some_and(|w| w.settings().send_on_save);
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
        send_on_save,
    };
    match s.updates.listing(&l).await {
        None => {}
        Some(Listed { tree: Err(p), .. }) => {
            view.checked = true;
            view.state = p.sentence();
        }
        Some(listed) => {
            view.checked = true;
            match s.updates.prepare(&l, &listed).await {
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
            Fate::Merged(bytes, exec) => {
                files.insert(f.path.clone(), (Arc::from(bytes.as_slice()), *exec));
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
    let Some(listed) = s.updates.listing(skill).await.filter(|l| l.tree.is_ok()) else {
        return Err("Check for updates first; nothing changed.".to_owned());
    };
    let prepared = s
        .updates
        .prepare(skill, &listed)
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
    let pin = prepared.local.pin.clone();
    let write: change::Apply = Box::new(move || {
        // The pin was checked before the git lock was taken; an edit or update committed
        // since would be written over, so the folder is read again here, under the lock.
        still_pinned(&root, &rel, &pin)?;
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

/// Refuses when the skill's folder is no longer the one the preview showed.
fn still_pinned(root: &Path, rel: &str, pin: &str) -> Result<(), String> {
    let now = read_local(&root.join(rel))?;
    if now.pin == pin {
        Ok(())
    } else {
        Err("The skill changed since the preview. Open it again.".to_owned())
    }
}

/// Replaces the skill's folder with `tree` (execute bits kept). Runs inside the change,
/// which puts the folder back from git if anything here fails.
fn write_folder(root: &Path, rel: &str, tree: &Tree) -> Result<(), String> {
    atlas_fs::library::remove_tree(root, rel)
        .map_err(|e| format!("Removing the old copy failed: {e}."))?;
    atlas_fs::library::write_tree(root, rel, tree)
        .map_err(|e| format!("Writing the update failed: {e}."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_changed_after_the_preview_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("skills/x");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("SKILL.md"), "one\n").unwrap();
        let pin = read_local(&folder).unwrap().pin;
        assert_eq!(still_pinned(dir.path(), "skills/x", &pin), Ok(()));
        // Committed meanwhile (an edit, or another update).
        std::fs::write(folder.join("SKILL.md"), "two\n").unwrap();
        let got = still_pinned(dir.path(), "skills/x", &pin);
        assert!(got.is_err_and(|m| m.contains("changed since the preview")));
        // A new file counts too.
        std::fs::write(folder.join("SKILL.md"), "one\n").unwrap();
        std::fs::write(folder.join("extra.md"), "x\n").unwrap();
        assert!(still_pinned(dir.path(), "skills/x", &pin).is_err());
    }
}
