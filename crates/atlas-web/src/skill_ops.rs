//! Skill library operations: create, delete and move a skill between homes. Each is one
//! change through [`crate::change`], so it follows the save rules and can be undone with
//! git. The placements file follows a skill when it moves or is deleted.
//!
//! CONTRACT (decision 0027): these signatures are shared by the placement page ("Use
//! in…" moves a skill between Global and a project) and the list page (bulk move and
//! delete). The library worker implements them; callers may rely on them as written.
//!
//! Public skills follow decision 0016: changes to the public repo are committed and never
//! pushed by Atlas (the owner pushes after a privacy check); the confirmation says so.
//!
//! Private and public skills share one placements namespace (`[global.<name>]`), because
//! atlasd links skills by name. Deleting or moving one global skill drops its placement
//! entry only when no other global home still has a skill of that name. The placements
//! file is read and written only inside the change's apply step.
//!
//! The `skills` CLI's record of where a skill came from (`atlas_fs::lock`) goes with it
//! when both locks involved sit in repos Atlas commits. A lock outside git (the global
//! one usually is, in `~/.agents`) is never written; [`preview_move`] and
//! [`preview_delete`] say so, for the confirmation page.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use atlas_core::catalog::edit::RepoKind;
use atlas_core::catalog::{PLACEMENTS_FILE, Placement, Placements, SkillHome};
use atlas_core::kernel::Slug;
use atlas_fs::library::{self, LibraryError, Tree, TreeFile};
use atlas_fs::lock::{self, LockFile, LockKind};
use axum::http::HeaderMap;

use crate::AppState;
use crate::change::{Apply, ChangeError, Outcome, RepoChange, commit_change};
use crate::views::Snapshot;

/// Creates `<home>/<name>/SKILL.md` with a frontmatter `name` and `description`, on for
/// every agent. Refuses a name that isn't a slug or is taken by any skill folder any agent
/// reads (in the store, a project, or a harness's own folder).
pub(crate) async fn create_skill(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    home: SkillHome,
    name: Slug,
    description: String,
) -> Result<Outcome, ChangeError> {
    authorise(s, headers, csrf)?;
    let description = check_description(&description).map_err(ChangeError::Refused)?;
    if matches!(home, SkillHome::Public) {
        return Err(refused(
            "New skills start private. Making one public is done by hand after a privacy check.",
        ));
    }
    let snap = s
        .snapshot()
        .await
        .map_err(|_| refused("Atlas couldn't read the store."))?;
    if let Some(at) = name_taken(&snap, name.as_str()) {
        return Err(ChangeError::Refused(format!(
            "A skill named {name} already exists ({at}). Pick another name; agents tell skills apart by name."
        )));
    }
    let st = s.clone();
    let plan = blocking(move || {
        let at = home_at(&st, &home)?;
        let rel = format!("{}/{name}", at.dir);
        let mut plan = Plan::new(format!("Create skill {name} in Atlas"));
        plan.touch(&at.kind, &at.root, &rel);
        plan.writes.push(Write::NewTree {
            root: at.root.clone(),
            rel,
            tree: Tree {
                files: vec![TreeFile {
                    rel: PathBuf::from("SKILL.md"),
                    bytes: skill_text(name.as_str(), &description).into_bytes(),
                    executable: false,
                }],
                skipped: Vec::new(),
            },
        });
        // A leftover entry for the same name would switch the new skill off somewhere.
        plan.placement(&st, PlacementOp::Forget { home, name });
        plan.close(&st);
        Ok(plan)
    })
    .await?;
    plan.commit(s, headers, csrf).await
}

/// Deletes skills' folders and their placement entries, as one change (one commit per
/// repo it touches). atlasd removes the links on each machine with the next update.
pub(crate) async fn delete_skills(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    skills: Vec<(SkillHome, Slug)>,
) -> Result<Outcome, ChangeError> {
    authorise(s, headers, csrf)?;
    let st = s.clone();
    let plan = blocking(move || plan_delete(&st, &skills)).await?;
    plan.commit(s, headers, csrf).await
}

/// Moves skills' folders to another home (Global ↔ a project, private ↔ public),
/// carrying each placement (per-agent `off` stays; `only-projects` is dropped when it
/// becomes a project skill) and the `skills` CLI's lock record when there is one. Never
/// merges or overwrites: a name already taken at the destination refuses the whole
/// change. Commits the destination repo first, then the source repos.
pub(crate) async fn move_skills(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    skills: Vec<(SkillHome, Slug)>,
    to: SkillHome,
) -> Result<Outcome, ChangeError> {
    authorise(s, headers, csrf)?;
    let st = s.clone();
    let plan = blocking(move || plan_move(&st, &skills, &to)).await?;
    plan.commit(s, headers, csrf).await
}

/// What a confirmation page shows before a change: one line per thing that changes, and
/// notes such as a lock file Atlas leaves alone.
#[derive(Debug, Default)]
pub(crate) struct Preview {
    pub lines: Vec<String>,
    pub notes: Vec<String>,
}

/// What deleting `skills` would do, or why it would be refused.
pub(crate) async fn preview_delete(
    s: &AppState,
    skills: Vec<(SkillHome, Slug)>,
) -> Result<Preview, String> {
    let st = s.clone();
    match blocking(move || plan_delete(&st, &skills)).await {
        Ok(plan) => Ok(plan.preview()),
        Err(ChangeError::Refused(m)) => Err(m),
        Err(_) => Err("Sign in again.".to_owned()),
    }
}

/// What moving `skills` to `to` would do, or why it would be refused.
pub(crate) async fn preview_move(
    s: &AppState,
    skills: Vec<(SkillHome, Slug)>,
    to: SkillHome,
) -> Result<Preview, String> {
    let st = s.clone();
    match blocking(move || plan_move(&st, &skills, &to)).await {
        Ok(plan) => Ok(plan.preview()),
        Err(ChangeError::Refused(m)) => Err(m),
        Err(_) => Err("Sign in again.".to_owned()),
    }
}

// ---------------------------------------------------------------- checks

fn refused(msg: &str) -> ChangeError {
    ChangeError::Refused(msg.to_owned())
}

fn authorise(s: &AppState, headers: &HeaderMap, csrf: &str) -> Result<(), ChangeError> {
    match &s.writer {
        None => Err(ChangeError::ReadOnly),
        Some(w) if w.authorised(headers, csrf) => Ok(()),
        Some(_) => Err(ChangeError::Forbidden),
    }
}

/// The longest description agents accept (Claude Code's limit).
pub(crate) const MAX_DESCRIPTION: usize = 1024;

/// A one-line description, trimmed.
pub(crate) fn check_description(text: &str) -> Result<String, String> {
    let t = text.trim();
    if t.is_empty() {
        return Err(
            "Add a one-line description: agents read it to decide when to use the skill."
                .to_owned(),
        );
    }
    if t.contains(['\n', '\r']) || t.chars().any(char::is_control) {
        return Err("Keep the description to one line.".to_owned());
    }
    if t.chars().count() > MAX_DESCRIPTION {
        return Err(format!(
            "Keep the description under {MAX_DESCRIPTION} characters."
        ));
    }
    Ok(t.to_owned())
}

/// Where `name` is already used by a skill folder any agent reads, if anywhere.
pub(crate) fn name_taken(snap: &Snapshot, name: &str) -> Option<String> {
    if let Some(s) = snap.catalog.skills.iter().find(|s| s.name.as_str() == name) {
        return Some(s.path.trim_end_matches("/SKILL.md").to_owned());
    }
    for h in &snap.harnesses {
        for d in &h.skill_dirs {
            if d.entries.iter().any(|e| e.name() == name) {
                return Some(format!("{}/{name}", d.path));
            }
        }
    }
    for p in &snap.projects {
        if p.harnesses
            .iter()
            .any(|h| h.skills.iter().any(|s| s == name))
        {
            return Some(format!("in {}", p.project));
        }
    }
    None
}

/// The new `SKILL.md`. The description is written as a JSON string, which YAML reads
/// back as the same text whatever it holds.
pub(crate) fn skill_text(name: &str, description: &str) -> String {
    let quoted = serde_json::to_string(description).unwrap_or_else(|_| "\"\"".to_owned());
    format!(
        "---\nname: {name}\ndescription: {quoted}\n---\n\n# {name}\n\nWrite the steps the agent should follow when it uses this skill.\n"
    )
}

// ---------------------------------------------------------------- plans

/// A home's repo and where its skill folders are inside it.
struct HomeAt {
    kind: RepoKind,
    root: PathBuf,
    dir: &'static str,
    shown: String,
}

fn home_at(s: &AppState, home: &SkillHome) -> Result<HomeAt, ChangeError> {
    let src = &s.sources;
    let (kind, root, dir) = match home {
        SkillHome::Private => (RepoKind::Knowledge, src.knowledge.clone(), "skills"),
        SkillHome::Public => (
            RepoKind::PublicSkills,
            src.public_skills
                .clone()
                .ok_or_else(|| refused("This hub has no public skills repo."))?,
            "",
        ),
        SkillHome::Project(p) => {
            let project = src
                .projects
                .iter()
                .find(|x| &x.name == p)
                .ok_or_else(|| refused("That project isn't registered."))?;
            (
                RepoKind::Project(p.clone()),
                project.root.clone(),
                ".agents/skills",
            )
        }
    };
    let shown = src.display(&if dir.is_empty() {
        root.clone()
    } else {
        root.join(dir)
    });
    Ok(HomeAt {
        kind,
        root,
        dir,
        shown,
    })
}

impl HomeAt {
    fn rel(&self, name: &str) -> String {
        if self.dir.is_empty() {
            name.to_owned()
        } else {
            format!("{}/{name}", self.dir)
        }
    }

    /// The skill's real folder, if it is one (not a link, not a file).
    fn has(&self, name: &str) -> bool {
        std::fs::symlink_metadata(self.root.join(self.rel(name))).is_ok_and(|m| m.is_dir())
    }

    fn occupied(&self, name: &str) -> bool {
        std::fs::symlink_metadata(self.root.join(self.rel(name))).is_ok()
    }
}

fn home_words(home: &SkillHome) -> String {
    match home {
        SkillHome::Private => "Global".to_owned(),
        SkillHome::Public => "Global (public)".to_owned(),
        SkillHome::Project(p) => p.to_string(),
    }
}

const PUBLIC_NOTE: &str = "The public skills repo is committed but never pushed by Atlas: check it for private details, then push it by hand.";

enum Write {
    NewTree {
        root: PathBuf,
        rel: String,
        tree: Tree,
    },
    Move {
        from_root: PathBuf,
        from_rel: String,
        to_root: PathBuf,
        to_rel: String,
    },
    Remove {
        root: PathBuf,
        rel: String,
    },
    File {
        root: PathBuf,
        rel: String,
        bytes: Vec<u8>,
    },
    Placements {
        knowledge: PathBuf,
        ops: Vec<PlacementOp>,
    },
}

/// One skill's entry in the placements file, applied inside the change.
#[derive(Clone, Debug)]
enum PlacementOp {
    /// The skill is new: drop any leftover entry for its name.
    Forget { home: SkillHome, name: Slug },
    /// The skill is gone: drop its entry, unless another global home still has a skill
    /// of that name (`keep`).
    ForgetUnlessShared {
        home: SkillHome,
        name: Slug,
        keep: bool,
    },
    /// The skill moved: its entry goes with it (and stays too when `keep`).
    Carry {
        from: SkillHome,
        to: SkillHome,
        name: Slug,
        keep: bool,
    },
}

impl PlacementOp {
    fn apply(&self, p: &mut Placements) {
        match self {
            Self::Forget { home, name } => p.remove(home, name),
            Self::ForgetUnlessShared { home, name, keep } => {
                if !keep {
                    p.remove(home, name);
                }
            }
            Self::Carry {
                from,
                to,
                name,
                keep,
            } => {
                // Private and public share one entry per name: nothing to carry.
                let global = |h: &SkillHome| !matches!(h, SkillHome::Project(_));
                if global(from) && global(to) {
                    return;
                }
                let carried = p.of(from, name).clone();
                if !keep {
                    p.remove(from, name);
                }
                p.set(to, name, carried);
            }
        }
    }
}

fn apply_placements(knowledge: &Path, ops: &[PlacementOp]) -> Result<(), LibraryError> {
    let (before, expected) = library::read_placements(knowledge)?;
    let mut now = before.clone();
    for op in ops {
        op.apply(&mut now);
    }
    if now != before {
        library::write_placements(knowledge, &now, &expected)?;
    }
    Ok(())
}

impl Write {
    fn run(self) -> Result<(), String> {
        let r = match self {
            Self::NewTree { root, rel, tree } => library::write_tree(&root, &rel, &tree),
            Self::Move {
                from_root,
                from_rel,
                to_root,
                to_rel,
            } => atlas_fs::skill_folder::move_folder(&from_root, &from_rel, &to_root, &to_rel),
            Self::Remove { root, rel } => library::remove_tree(&root, &rel),
            Self::File { root, rel, bytes } => library::write_file(&root, &rel, &bytes),
            Self::Placements { knowledge, ops } => apply_placements(&knowledge, &ops),
        };
        r.map_err(|e| {
            let mut t = e.to_string();
            if let Some(first) = t.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            format!("{t}.")
        })
    }
}

struct Plan {
    repos: Vec<RepoChange>,
    writes: Vec<Write>,
    placements_at: Option<usize>,
    message: String,
    lines: Vec<String>,
    notes: Vec<String>,
}

impl Plan {
    fn new(message: String) -> Self {
        Self {
            repos: Vec::new(),
            writes: Vec::new(),
            placements_at: None,
            message,
            lines: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// Adds `rel` of the repo at `root` to the change. Repos commit in the order they are
    /// first touched, so callers touch the repo that gains files first.
    fn touch(&mut self, kind: &RepoKind, root: &Path, rel: &str) {
        let rel = PathBuf::from(rel);
        if let Some(r) = self.repos.iter_mut().find(|r| r.root == root) {
            if !r.paths.contains(&rel) {
                r.paths.push(rel);
            }
        } else {
            self.repos.push(RepoChange {
                kind: kind.clone(),
                root: root.to_path_buf(),
                paths: vec![rel],
                message: self.message.clone(),
            });
        }
    }

    /// Records a placements change. The file is read and written inside the change, so
    /// it is always one of the knowledge repo's paths (committed only if it changed).
    fn placement(&mut self, s: &AppState, op: PlacementOp) {
        let knowledge = s.sources.knowledge.clone();
        if let Some(Write::Placements { ops, .. }) = self
            .writes
            .iter_mut()
            .find(|w| matches!(w, Write::Placements { .. }))
        {
            ops.push(op);
            return;
        }
        self.placements_at = Some(self.writes.len());
        self.writes.push(Write::Placements {
            knowledge,
            ops: vec![op],
        });
    }

    /// Touches the placements file last, after the repos that gain and lose folders.
    fn close(&mut self, s: &AppState) {
        if self.placements_at.is_some() {
            self.touch(&RepoKind::Knowledge, &s.sources.knowledge, PLACEMENTS_FILE);
        }
        if self.repos.iter().any(|r| r.kind == RepoKind::PublicSkills) {
            self.notes.push(PUBLIC_NOTE.to_owned());
        }
    }

    fn preview(self) -> Preview {
        Preview {
            lines: self.lines,
            notes: self.notes,
        }
    }

    async fn commit(
        self,
        s: &AppState,
        headers: &HeaderMap,
        csrf: &str,
    ) -> Result<Outcome, ChangeError> {
        if self.repos.is_empty() {
            return Ok(Outcome::Unchanged);
        }
        let writes = self.writes;
        let apply: Apply = Box::new(move || {
            for w in writes {
                w.run()?;
            }
            Ok(())
        });
        commit_change(s, headers, csrf, self.repos, apply).await
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ChangeError> + Send + 'static,
) -> Result<T, ChangeError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|_| refused("Atlas stopped unexpectedly; nothing changed."))?
}

/// Where a home's `skills` CLI lock is, and whether Atlas commits it.
#[derive(Clone, Debug)]
struct LockAt {
    kind: LockKind,
    abs: PathBuf,
    shown: String,
    /// The repo it is in, when that is a repo Atlas commits.
    repo: Option<(RepoKind, PathBuf, String)>,
}

fn lock_at(s: &AppState, home: &SkillHome) -> Option<LockAt> {
    let src = &s.sources;
    match home {
        SkillHome::Project(p) => {
            let project = src.projects.iter().find(|x| &x.name == p)?;
            let abs = lock::project_lock_path(&project.root);
            Some(LockAt {
                kind: LockKind::Project,
                shown: src.display(&abs),
                abs,
                repo: Some((
                    RepoKind::Project(p.clone()),
                    project.root.clone(),
                    lock::PROJECT_LOCK_FILE.to_owned(),
                )),
            })
        }
        SkillHome::Private | SkillHome::Public => {
            let abs = s.updates.global_lock().to_path_buf();
            let shown = src.display(&abs);
            let mut repos: Vec<(RepoKind, PathBuf)> =
                vec![(RepoKind::Knowledge, src.knowledge.clone())];
            if let Some(p) = &src.public_skills {
                repos.push((RepoKind::PublicSkills, p.clone()));
            }
            repos.extend(
                src.projects
                    .iter()
                    .map(|p| (RepoKind::Project(p.name.clone()), p.root.clone())),
            );
            let parent = abs.parent().and_then(|p| std::fs::canonicalize(p).ok());
            let file = abs.file_name().map(PathBuf::from);
            let repo = parent.zip(file).and_then(|(parent, file)| {
                repos.into_iter().find_map(|(kind, root)| {
                    let root = std::fs::canonicalize(&root).ok()?;
                    let rel = parent.strip_prefix(&root).ok()?.join(&file);
                    let plain = rel
                        .components()
                        .all(|c| matches!(c, std::path::Component::Normal(_)));
                    (plain && !rel.starts_with(".git"))
                        .then(|| (kind, root, rel.to_string_lossy().into_owned()))
                })
            });
            Some(LockAt {
                kind: LockKind::Global,
                abs,
                shown,
                repo,
            })
        }
    }
}

/// A lock as read for a plan.
#[derive(Debug)]
enum ReadLock {
    Missing,
    /// There is a file Atlas won't rewrite (it doesn't parse, is old, or is a link).
    Unusable,
    Found(LockFile),
}

/// Locks read so far in one plan, edited in memory and written once at the end.
#[derive(Default)]
struct Locks {
    read: BTreeMap<PathBuf, (LockAt, ReadLock, bool)>,
}

impl Locks {
    fn get(&mut self, at: &LockAt) -> &mut (LockAt, ReadLock, bool) {
        self.read.entry(at.abs.clone()).or_insert_with(|| {
            let read = match lock::read_lock(&at.abs, at.kind) {
                Ok(None) => ReadLock::Missing,
                Ok(Some(l)) => ReadLock::Found(l),
                Err(_) => ReadLock::Unusable,
            };
            (at.clone(), read, false)
        })
    }

    /// Writes every lock that changed. Locks that gain records go in first, as their
    /// repos were touched first.
    fn finish(self, plan: &mut Plan) {
        for (_, (at, read, changed)) in self.read {
            let (true, ReadLock::Found(lock), Some((kind, root, rel))) = (changed, read, at.repo)
            else {
                continue;
            };
            plan.touch(&kind, &root, &rel);
            plan.writes.push(Write::File {
                root,
                rel,
                bytes: lock.text().into_bytes(),
            });
        }
    }
}

fn source_words(lock: &LockFile, name: &str) -> String {
    lock.records
        .get(name)
        .and_then(lock::Record::github)
        .map_or_else(String::new, |g| format!(" (from {})", g.slug()))
}

/// Whether the other global home (private for public, public for private) has a skill
/// named `name` that this change leaves in place, so their shared entry stays.
fn shared_global(s: &AppState, home: &SkillHome, name: &Slug, batch: &[(SkillHome, Slug)]) -> bool {
    let other = match home {
        SkillHome::Private => SkillHome::Public,
        SkillHome::Public => SkillHome::Private,
        SkillHome::Project(_) => return false,
    };
    let leaves = !batch.iter().any(|(h, n)| *h == other && n == name);
    leaves && home_at(s, &other).is_ok_and(|o| o.occupied(name.as_str()))
}

fn plan_delete(s: &AppState, skills: &[(SkillHome, Slug)]) -> Result<Plan, ChangeError> {
    if skills.is_empty() {
        return Err(refused("Pick at least one skill."));
    }
    let names: Vec<&str> = skills.iter().map(|(_, n)| n.as_str()).collect();
    let mut plan = Plan::new(format!("Delete {} in Atlas", list_names(&names)));
    let (placed, _) = library::read_placements(&s.sources.knowledge).unwrap_or_default();
    let mut locks = Locks::default();
    let mut entries = false;
    for (home, name) in skills {
        let at = home_at(s, home)?;
        if !at.has(name.as_str()) {
            return Err(ChangeError::Refused(format!(
                "{name} isn't a folder in {} any more. Nothing changed.",
                at.shown
            )));
        }
        let rel = at.rel(name.as_str());
        plan.touch(&at.kind, &at.root, &rel);
        plan.writes.push(Write::Remove {
            root: at.root.clone(),
            rel,
        });
        plan.lines.push(format!("{}/{name}", at.shown));
        let keep = shared_global(s, home, name, skills);
        entries |= !keep && *placed.of(home, name) != Placement::default();
        plan.placement(
            s,
            PlacementOp::ForgetUnlessShared {
                home: home.clone(),
                name: name.clone(),
                keep,
            },
        );
        if keep {
            continue;
        }
        if let Some(lat) = lock_at(s, home) {
            let (lat, read, changed) = locks.get(&lat);
            if let ReadLock::Found(l) = read
                && l.raw(name.as_str()).is_some()
            {
                let from = source_words(l, name.as_str());
                if lat.repo.is_some() && l.writable() {
                    l.remove(name.as_str());
                    *changed = true;
                    plan.lines.push(format!(
                        "The skills CLI's record of {name}{from} in {}",
                        lat.shown
                    ));
                } else {
                    plan.notes.push(format!(
                        "The skills CLI still lists {name}{from} in {}, which {}; Atlas leaves it as it is.",
                        lat.shown,
                        if lat.repo.is_some() {
                            "has a version Atlas doesn't write"
                        } else {
                            "isn't in a repo Atlas commits"
                        }
                    ));
                }
            }
        }
    }
    if entries {
        plan.lines
            .push(format!("Their switches in {PLACEMENTS_FILE}"));
    }
    // Folders first, then lock records, then placements: the order writes happen in.
    locks.finish(&mut plan);
    plan.close(s);
    Ok(plan)
}

fn plan_move(
    s: &AppState,
    skills: &[(SkillHome, Slug)],
    to: &SkillHome,
) -> Result<Plan, ChangeError> {
    if skills.is_empty() {
        return Err(refused("Pick at least one skill."));
    }
    let dest = home_at(s, to)?;
    let names: Vec<&str> = skills.iter().map(|(_, n)| n.as_str()).collect();
    let mut plan = Plan::new(format!(
        "Move {} to {} in Atlas",
        list_names(&names),
        home_words(to)
    ));
    let mut seen = std::collections::BTreeSet::new();
    // Touch the destination first, so it commits first.
    for (_, name) in skills {
        plan.touch(&dest.kind, &dest.root, &dest.rel(name.as_str()));
    }
    let (placed, _) = library::read_placements(&s.sources.knowledge).unwrap_or_default();
    let mut entries = false;
    let mut locks = Locks::default();
    let to_lock = lock_at(s, to);
    let now = iso_now();
    for (from, name) in skills {
        if from == to {
            return Err(ChangeError::Refused(format!(
                "{name} is already in {}.",
                home_words(to)
            )));
        }
        if !seen.insert(name.clone()) {
            return Err(ChangeError::Refused(format!(
                "Two of the chosen skills are named {name}; they can't both move to {}.",
                home_words(to)
            )));
        }
        let src = home_at(s, from)?;
        if !src.has(name.as_str()) {
            return Err(ChangeError::Refused(format!(
                "{name} isn't a folder in {} any more. Nothing changed.",
                src.shown
            )));
        }
        if dest.occupied(name.as_str()) {
            return Err(ChangeError::Refused(format!(
                "{}/{name} already exists. Atlas never merges or overwrites a skill; rename or delete one first.",
                dest.shown
            )));
        }
        // Private and public skills share one namespace: a skill moving into one of them
        // can't take a name the other holds (unless that is the skill itself moving).
        if to.scope() == atlas_core::kernel::Scope::Global
            && shared_global(s, to, name, &[(from.clone(), name.clone())])
        {
            return Err(ChangeError::Refused(format!(
                "Another global skill is already named {name}. Rename one first."
            )));
        }
        let from_rel = src.rel(name.as_str());
        plan.writes.push(Write::Move {
            from_root: src.root.clone(),
            from_rel: from_rel.clone(),
            to_root: dest.root.clone(),
            to_rel: dest.rel(name.as_str()),
        });
        plan.touch(&src.kind, &src.root, &from_rel);
        plan.lines
            .push(format!("{}/{name} → {}/{name}", src.shown, dest.shown));
        let keep = shared_global(s, from, name, skills);
        entries |= *placed.of(from, name) != Placement::default();
        plan.placement(
            s,
            PlacementOp::Carry {
                from: from.clone(),
                to: to.clone(),
                name: name.clone(),
                keep,
            },
        );
        if let (Some(fl), Some(tl)) = (lock_at(s, from), to_lock.as_ref())
            && !keep
        {
            carry_record(&mut plan, &mut locks, &fl, tl, &src, name, &now);
        }
    }
    if entries {
        plan.lines
            .push(format!("Their switches go with them in {PLACEMENTS_FILE}"));
    }
    locks.finish(&mut plan);
    plan.close(s);
    Ok(plan)
}

/// Moves `name`'s record from lock `fl` to lock `tl` when both are in repos Atlas
/// commits and both can be written; otherwise leaves both and says why.
fn carry_record(
    plan: &mut Plan,
    locks: &mut Locks,
    fl: &LockAt,
    tl: &LockAt,
    src: &HomeAt,
    name: &Slug,
    now: &str,
) {
    let (entry, from_words) = match &locks.get(fl).1 {
        ReadLock::Found(l) => match l.raw(name.as_str()) {
            Some(e) => (e.clone(), source_words(l, name.as_str())),
            None => return,
        },
        _ => return,
    };
    let leave = |plan: &mut Plan, why: String| {
        plan.notes.push(format!(
            "The skills CLI's record of {name}{from_words} stays in {}{why}, so Atlas changes neither lock.",
            fl.shown
        ));
    };
    if fl.abs == tl.abs {
        return;
    }
    let outside = [fl, tl].into_iter().find(|l| l.repo.is_none());
    if let Some(o) = outside {
        let why = if o.abs == fl.abs {
            ", which isn't in a repo Atlas commits".to_owned()
        } else {
            format!(": {} isn't in a repo Atlas commits", o.shown)
        };
        leave(plan, why);
        return;
    }
    let from_ok = matches!(&locks.get(fl).1, ReadLock::Found(l) if l.writable());
    let to_ok = match &locks.get(tl).1 {
        ReadLock::Missing => true,
        ReadLock::Found(l) => l.writable(),
        ReadLock::Unusable => false,
    };
    if !from_ok || !to_ok {
        let bad = if from_ok { tl } else { fl };
        leave(
            plan,
            format!(": {} isn't a lock Atlas can rewrite safely", bad.shown),
        );
        return;
    }
    let hashes = (fl.kind == LockKind::Global && tl.kind == LockKind::Project)
        .then(|| lock::hash_folder(&src.root.join(src.rel(name.as_str()))))
        .flatten();
    let converted = lock::convert_record(&entry, fl.kind, tl.kind, hashes.as_ref(), now);
    if let Some(record) = converted {
        let slot = locks.get(tl);
        if matches!(slot.1, ReadLock::Missing) {
            slot.1 = ReadLock::Found(LockFile::fresh(&tl.abs, tl.kind));
        }
        if let ReadLock::Found(l) = &mut slot.1 {
            l.insert(name.as_str(), record);
            slot.2 = true;
        }
        plan.lines.push(format!(
            "The skills CLI's record of {name}{from_words}: {} → {}",
            fl.shown, tl.shown
        ));
    } else {
        plan.notes.push(format!(
            "The skills CLI's record of {name}{from_words} can't be carried to {} (its files no longer match what was installed), so it is dropped; npx skills will treat the skill as added by hand.",
            tl.shown
        ));
    }
    let slot = locks.get(fl);
    if let ReadLock::Found(l) = &mut slot.1 {
        l.remove(name.as_str());
        slot.2 = true;
    }
}

fn list_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a} and {b}"),
        [a, b, c] => format!("{a}, {b} and {c}"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} more", rest.len()),
    }
}

/// Now as the CLI writes it: `2026-11-27T10:00:00.000Z`.
fn iso_now() -> String {
    let secs = crate::clients::now();
    let day = crate::edit::day(secs);
    let t = secs % 86_400;
    format!(
        "{day}T{:02}:{:02}:{:02}.000Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_are_one_trimmed_line() {
        assert_eq!(check_description("  Reads PDFs ").unwrap(), "Reads PDFs");
        assert!(check_description("").is_err());
        assert!(check_description("a\nb").is_err());
        assert!(check_description(&"x".repeat(MAX_DESCRIPTION + 1)).is_err());
    }

    #[test]
    fn new_skill_text_quotes_the_description() {
        let text = skill_text("pdf", "Reads \"PDFs\": fast");
        assert!(
            text.starts_with("---\nname: pdf\ndescription: \"Reads \\\"PDFs\\\": fast\"\n---\n"),
            "{text}"
        );
        assert!(text.contains("\n# pdf\n"));
    }

    fn state(home: &Path) -> AppState {
        let mk = |p: &Path| {
            std::fs::create_dir_all(p).unwrap();
        };
        mk(&home.join("Knowledge/skills/pdf"));
        std::fs::write(home.join("Knowledge/skills/pdf/SKILL.md"), "x").unwrap();
        mk(&home.join("shop/.agents/skills/stock"));
        std::fs::write(home.join("shop/.agents/skills/stock/SKILL.md"), "x").unwrap();
        let sources = atlas_fs::Sources {
            home: home.to_path_buf(),
            knowledge: home.join("Knowledge"),
            public_skills: None,
            projects: vec![atlas_fs::Project {
                name: Slug::new("shop").unwrap(),
                root: home.join("shop"),
            }],
        };
        AppState::new(sources, "hub", Vec::new(), None)
            .with_github_stand_in("http://127.0.0.1:9", home.join("outside/.skill-lock.json"))
    }

    #[test]
    fn a_move_commits_the_destination_first_and_placements_last() {
        let tmp = tempfile::tempdir().unwrap();
        let s = state(tmp.path());
        let shop = SkillHome::Project(Slug::new("shop").unwrap());
        let pdf = Slug::new("pdf").unwrap();
        let plan = plan_move(&s, &[(SkillHome::Private, pdf.clone())], &shop).unwrap();
        let kinds: Vec<&RepoKind> = plan.repos.iter().map(|r| &r.kind).collect();
        assert_eq!(
            kinds,
            [
                &RepoKind::Project(Slug::new("shop").unwrap()),
                &RepoKind::Knowledge
            ]
        );
        assert_eq!(
            plan.repos[1].paths,
            [PathBuf::from("skills/pdf"), PathBuf::from(PLACEMENTS_FILE)]
        );
        let stock = Slug::new("stock").unwrap();
        let back = plan_move(&s, &[(shop.clone(), stock)], &SkillHome::Private).unwrap();
        let kinds: Vec<&RepoKind> = back.repos.iter().map(|r| &r.kind).collect();
        assert_eq!(
            kinds,
            [
                &RepoKind::Knowledge,
                &RepoKind::Project(Slug::new("shop").unwrap())
            ]
        );
        // Into the same home, or onto a taken name, is refused.
        assert!(
            plan_move(
                &s,
                &[(SkillHome::Private, pdf.clone())],
                &SkillHome::Private
            )
            .is_err()
        );
        std::fs::create_dir_all(tmp.path().join("shop/.agents/skills/pdf")).unwrap();
        assert!(plan_move(&s, &[(SkillHome::Private, pdf)], &shop).is_err());
    }

    #[test]
    fn placements_follow_the_rules_of_one_global_namespace() {
        use atlas_core::kernel::HarnessKind;
        let name = Slug::new("pdf").unwrap();
        let shop = SkillHome::Project(Slug::new("shop").unwrap());
        let mut off = Placement::default();
        off.off.insert(HarnessKind::Codex);
        off.only_projects = Some(std::collections::BTreeSet::from([
            Slug::new("shop").unwrap()
        ]));
        let mut p = Placements::default();
        p.set(&SkillHome::Private, &name, off);
        let mut moved = p.clone();
        PlacementOp::Carry {
            from: SkillHome::Private,
            to: shop.clone(),
            name: name.clone(),
            keep: false,
        }
        .apply(&mut moved);
        assert!(moved.global.is_empty());
        let carried = moved.of(&shop, &name);
        assert!(!carried.wants(HarnessKind::Codex));
        assert!(
            carried.only_projects.is_none(),
            "only-projects is dropped for a project skill"
        );
        let mut shared = p.clone();
        PlacementOp::ForgetUnlessShared {
            home: SkillHome::Private,
            name: name.clone(),
            keep: true,
        }
        .apply(&mut shared);
        assert_eq!(shared, p, "a public skill of the same name keeps the entry");
        let mut between = p.clone();
        PlacementOp::Carry {
            from: SkillHome::Private,
            to: SkillHome::Public,
            name,
            keep: false,
        }
        .apply(&mut between);
        assert_eq!(between, p, "private and public share the entry");
    }

    #[test]
    fn names_read_as_a_list() {
        assert_eq!(list_names(&["a"]), "a");
        assert_eq!(list_names(&["a", "b", "c", "d"]), "a, b and 2 more");
    }
}
