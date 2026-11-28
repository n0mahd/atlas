//! One project checkout on this machine (decision 0027): links for the project's own
//! skills and for global skills used only in some projects, in the folders agents read
//! inside the repo; Atlas's block in `.git/info/exclude`, so the owner's `git status`
//! stays clean; and Claude Code's memory folder for the repo, linked into the owner's
//! knowledge checkout. Atlas never writes a path git tracks, and in a checkout removes
//! only links its block lists.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use atlas_core::catalog::SkillHome;
use atlas_core::kernel::Slug;
use atlas_core::release::remote_key;
use atlas_fs::library::read_tree;
use atlas_harness::Layout;
use atlas_harness::placement::Reach;
use atlas_harness::skills::{SkillConflict, SkillOp};

use crate::checkout::{self, Checkout};
use crate::desired::{self, Desired};
use crate::place::{self, Placer};
use crate::plan::{self, Action, Digests, MemoryStore, Plan};
use crate::{display, tree};

/// The project's own skills folder: it holds the skills themselves.
pub(crate) const OWN: &str = ".agents/skills";

/// Where one checkout's skills go.
pub(crate) struct Placed {
    pub placed: place::Placed,
    pub conflicts: Vec<String>,
}

/// Folders inside the repo that show `name` to their readers whatever Atlas does: the
/// project's own `.agents/skills`, and any agent folder where git tracks an entry of that
/// name (a committed link or copy Atlas must not touch).
fn holders(harnesses: &[&'static Layout], co: &Checkout, name: &str) -> Vec<&'static str> {
    let mut own = vec![OWN];
    let Ok(tracked) = &co.tracked else {
        return own;
    };
    for dir in harnesses.iter().flat_map(|l| l.project_skill_dirs) {
        if !own.contains(dir) && tracked.covers(&format!("{dir}/{name}")) {
            own.push(dir);
        }
    }
    own
}

/// Where the project's own skills and the global skills it uses go in `co`.
pub(crate) fn place(harnesses: &[&'static Layout], desired: &Desired, co: &Checkout) -> Placed {
    let mut placer = Placer::new(harnesses, Reach::Project, &co.root);
    let mut conflicts = Vec::new();
    let own_skills = desired::skills_in(&co.root.join(OWN), &mut conflicts);
    let home = SkillHome::Project(co.project.clone());
    for (name, source) in &own_skills {
        let off = Slug::new(name.as_str()).map_or_else(
            |_| BTreeSet::new(),
            |s| desired.placements.of(&home, &s).off.clone(),
        );
        placer.add(name, source, &off, &holders(harnesses, co, name));
    }
    for (name, source) in &desired.skills {
        let placement = desired.placement(name);
        if !placement.linked_into(&co.project) {
            continue;
        }
        if own_skills.contains_key(name) {
            conflicts.push(format!(
                "{} has its own skill {name}, so the global skill of that name isn't linked there",
                co.root.display()
            ));
            continue;
        }
        placer.add(name, source, &placement.off, &[]);
    }
    Placed {
        placed: placer.placed,
        conflicts,
    }
}

/// `target` as seen from `dir` (`../../.agents/skills/x`), for links that must keep
/// working wherever the checkout is moved or cloned.
fn relative(dir: &Path, target: &Path) -> PathBuf {
    let from: Vec<Component> = dir.components().collect();
    let to: Vec<Component> = target.components().collect();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in common..from.len() {
        out.push("..");
    }
    for c in &to[common..] {
        out.push(c);
    }
    out
}

/// One folder inside a checkout, for turning link operations into actions there.
struct Rel<'a> {
    co: &'a Checkout,
    tracked: &'a checkout::Tracked,
    listed: &'a BTreeSet<String>,
    folder: &'a str,
    dir: &'a Path,
}

impl Rel<'_> {
    /// Targets inside the checkout become relative, so links survive a move or clone.
    fn target(&self, t: PathBuf) -> PathBuf {
        if t.starts_with(&self.co.root) {
            relative(self.dir, &t)
        } else {
            t
        }
    }

    /// The action for `op`, None to skip it, or the name when git tracks its path. Only
    /// links Atlas listed in its exclude block are ever removed.
    fn action(&self, op: SkillOp) -> Result<Option<Action>, String> {
        let tracked = |name: &str| self.tracked.covers(&format!("{}/{name}", self.folder));
        let (name, action) = match op {
            SkillOp::RemoveLink { name, target } => {
                let listed = self.listed.contains(&format!("{}/{name}", self.folder));
                return Ok((listed && !tracked(&name)).then(|| Action::RemoveLink {
                    path: self.dir.join(&name),
                    target,
                }));
            }
            SkillOp::CreateLink { name, target } => {
                let action = Action::CreateLink {
                    path: self.dir.join(&name),
                    target: self.target(target),
                };
                (name, action)
            }
            SkillOp::ReplaceLink { name, from, to } => {
                let action = Action::ReplaceLink {
                    path: self.dir.join(&name),
                    from,
                    to: self.target(to),
                };
                (name, action)
            }
            SkillOp::AdoptDir {
                name,
                target,
                digest,
            } => {
                let action = Action::AdoptDir {
                    path: self.dir.join(&name),
                    target: self.target(target),
                    digest,
                    // Planned only for an exact copy (same files, bytes and execute bits).
                    keep: false,
                };
                (name, action)
            }
        };
        if tracked(&name) {
            Err(name)
        } else {
            Ok(Some(action))
        }
    }
}

fn conflict_name(c: &SkillConflict) -> &str {
    match c {
        SkillConflict::NotALink { name }
        | SkillConflict::ForeignLink { name, .. }
        | SkillConflict::SourceOutsideRoots { name, .. } => name,
    }
}

/// Plans `co`'s links and its exclude block. `switch_files` are the files inside it that
/// Atlas switches skills off in (they go in the block too).
pub(crate) fn links(
    plan: &mut Plan,
    home: &Path,
    managed: &[PathBuf],
    co: &Checkout,
    placed: &Placed,
    switch_files: &BTreeSet<String>,
    digests: &mut Digests,
) {
    let shown = display(home, &co.root);
    plan.conflicts.extend(placed.conflicts.iter().cloned());
    plan.conflicts
        .extend(placed.placed.stuck.iter().map(|s| format!("{shown}: {s}")));
    let tracked = match &co.tracked {
        Ok(t) => t,
        Err(e) => {
            plan.conflicts.push(format!("{shown} was left alone: {e}"));
            return;
        }
    };
    let exclude = match checkout::read_exclude(&co.root) {
        Ok(text) => text,
        Err(e) => {
            plan.conflicts.push(format!("{shown} was left alone: {e}"));
            return;
        }
    };
    let listed = checkout::listed(&exclude);
    let skill_dirs: BTreeSet<&str> = atlas_harness::layouts()
        .iter()
        .flat_map(|l| l.project_skill_dirs.iter().copied())
        .collect();
    let mut folders: Vec<(String, BTreeMap<String, PathBuf>)> = placed
        .placed
        .folders
        .iter()
        .map(|(f, m)| ((*f).to_owned(), m.clone()))
        .collect();
    // A folder no skill goes in any more still loses the links Atlas listed there.
    for rel in &listed {
        if let Some((folder, _)) = rel.rsplit_once('/')
            && skill_dirs.contains(folder)
            && !folders.iter().any(|(f, _)| f == folder)
        {
            folders.push((folder.to_owned(), BTreeMap::new()));
        }
    }

    let mut keep: BTreeSet<String> = switch_files.clone();
    let mut actions = Vec::new();
    for (folder, wanted) in &folders {
        let dir = co.root.join(folder);
        let mut roots = managed.to_vec();
        if folder != OWN {
            roots.push(co.root.join(OWN));
        }
        let Some(planned) = plan::folder_plan(plan, home, &dir, wanted, &roots, digests, "skill")
        else {
            continue;
        };
        let dir_shown = display(home, &dir);
        let mut refused = BTreeSet::new();
        for c in planned.conflicts {
            refused.insert(conflict_name(&c).to_owned());
            plan.conflicts
                .push(plan::link_conflict(home, &dir_shown, "skill", c));
        }
        let rel = Rel {
            co,
            tracked,
            listed: &listed,
            folder,
            dir: &dir,
        };
        for op in planned.ops {
            match rel.action(op) {
                Ok(Some(action)) => actions.push(action),
                Ok(None) => {}
                Err(name) => {
                    plan.conflicts.push(format!(
                        "{dir_shown}/{name} is tracked by git, so Atlas left it alone"
                    ));
                    refused.insert(name);
                }
            }
        }
        for name in wanted.keys().filter(|n| !refused.contains(*n)) {
            keep.insert(format!("{folder}/{name}"));
        }
    }
    // The block first, so a new link never shows in `git status`. Entries are only ever
    // added: the hub keeps its own there, and a line for a link that is gone is harmless.
    if let Some(text) = checkout::with_paths(&exclude, &keep) {
        let info = co.root.join(".git/info");
        plan.ensure_dir(&info);
        plan.actions.push(Action::WriteFile {
            path: info.join("exclude"),
            bytes: text.into_bytes(),
            why: "keep Atlas's links out of git status",
            // Lines are only added to the owner's exclude file.
            keep: false,
        });
    }
    plan.actions.extend(actions);
}

/// Resolves `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

/// Makes Claude Code's memory folder for `co` a link to
/// `<store>/memory/projects/<project>/`, so new notes go straight to the owner's git
/// (decision 0008). A real folder becomes the link only when every note in it is already
/// there byte for byte (the hub's import put them there) and it holds nothing else.
pub(crate) fn memory(
    plan: &mut Plan,
    home: &Path,
    managed: &[PathBuf],
    co: &Checkout,
    store: &MemoryStore,
) {
    let store = match store {
        MemoryStore::None => return,
        MemoryStore::Unusable(why) => {
            if !plan.notes.contains(why) {
                plan.notes.push(why.clone());
            }
            return;
        }
        MemoryStore::At(store) => store,
    };
    let link = atlas_harness::claude_project_memory(home, &co.root);
    let target = store.join("memory/projects").join(co.project.as_str());
    let shown = display(home, &link);
    let target_ok = fs::symlink_metadata(&target).is_ok_and(|m| m.is_dir());
    let parent = link.parent().unwrap_or(home);
    match fs::symlink_metadata(&link) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if target_ok {
                plan.ensure_dir(parent);
                plan.actions.push(Action::CreateLink { path: link, target });
            }
        }
        Err(e) => plan.conflicts.push(format!("can't read {shown}: {e}")),
        Ok(m) if m.file_type().is_symlink() => {
            let Ok(raw) = fs::read_link(&link) else {
                plan.conflicts.push(format!("can't read {shown}"));
                return;
            };
            let at = normalize(&parent.join(raw));
            if at == normalize(&target) {
                return;
            }
            let ours = managed
                .iter()
                .chain([store])
                .any(|r| at.starts_with(normalize(r)));
            if !ours {
                plan.conflicts.push(format!(
                    "{shown} links to {}, which Atlas doesn't manage, so it was left alone",
                    display(home, &at)
                ));
            } else if target_ok {
                plan.actions.push(Action::ReplaceLink {
                    path: link,
                    from: at,
                    to: target,
                });
            }
        }
        Ok(m) if m.is_dir() => adopt_notes(plan, &shown, link, target, target_ok),
        Ok(_) => plan
            .conflicts
            .push(format!("{shown} isn't a folder, so Atlas left it alone")),
    }
}

fn adopt_notes(plan: &mut Plan, shown: &str, link: PathBuf, target: PathBuf, target_ok: bool) {
    let Some((tree, digest)) = tree::exact(&link) else {
        plan.conflicts.push(format!(
            "{shown} holds links or other things besides notes, so Atlas left it alone"
        ));
        return;
    };
    let plain_note =
        |rel: &Path| rel.components().count() == 1 && rel.extension().is_some_and(|e| e == "md");
    if !tree.files.iter().all(|f| plain_note(&f.rel)) {
        plan.conflicts.push(format!(
            "{shown} holds files that aren't notes, so Atlas left it alone"
        ));
        return;
    }
    let theirs: BTreeMap<PathBuf, (Vec<u8>, bool)> = target_ok
        .then(|| read_tree(&target).ok())
        .flatten()
        .map_or_else(BTreeMap::new, |t| {
            t.files
                .into_iter()
                .map(|f| (f.rel, (f.bytes, f.executable)))
                .collect()
        });
    let missing = tree
        .files
        .iter()
        .filter(|f| theirs.get(&f.rel).is_none_or(|(b, _)| *b != f.bytes))
        .count();
    // Every note's text is in the store; a different execute bit is all the link
    // wouldn't show, and then the folder is kept as it was too.
    let keep = tree
        .files
        .iter()
        .any(|f| theirs.get(&f.rel).is_some_and(|(_, x)| *x != f.executable));
    match missing {
        0 if target_ok => plan.actions.push(Action::AdoptDir {
            path: link,
            target,
            digest,
            keep,
        }),
        0 => {}
        1 => plan.conflicts.push(format!(
            "1 note in {shown} isn't in Atlas yet, so the folder stays; bring it in from the \
             hub's Memory page"
        )),
        n => plan.conflicts.push(format!(
            "{n} notes in {shown} aren't in Atlas yet, so the folder stays; bring them in from \
             the hub's Memory page"
        )),
    }
}

/// The owner's own checkout of the knowledge repo, for memory links: `memory_store` from
/// `client.toml` when set, else the first adopted folder inside such a checkout. It must
/// be a git checkout whose `origin` is the knowledge repo's remote, so new notes land in
/// a repo the owner's sync commits and pushes. Atlas's local copy never qualifies: it is
/// reset to the signed commit on every pass, which would delete new notes.
#[must_use]
pub fn memory_store(
    explicit: Option<PathBuf>,
    adopted: &[PathBuf],
    home: &Path,
    knowledge_remote: Option<&str>,
) -> MemoryStore {
    let Some(remote) = knowledge_remote else {
        return MemoryStore::None;
    };
    let key = remote_key(remote);
    let is_store = |dir: &Path| checkout::origin(dir).is_some_and(|o| remote_key(&o) == key);
    if let Some(dir) = explicit {
        return if is_store(&dir) {
            MemoryStore::At(dir)
        } else {
            MemoryStore::Unusable(format!(
                "memory_store {} in client.toml isn't a checkout of the knowledge repo, so \
                 Claude Code's memory folders weren't linked",
                display(home, &dir)
            ))
        };
    }
    for root in adopted {
        let mut at = Some(root.as_path());
        while let Some(dir) = at {
            if dir == home || dir.parent().is_none() {
                break;
            }
            if is_store(dir) {
                return MemoryStore::At(dir.to_path_buf());
            }
            at = dir.parent();
        }
    }
    MemoryStore::Unusable(
        "Claude Code's memory folders weren't linked: set memory_store in client.toml to \
         your own checkout of the knowledge repo (Atlas's local copy is read-only)"
            .to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_links_climb_to_the_common_folder() {
        assert_eq!(
            relative(
                Path::new("/h/p/shop/.claude/skills"),
                Path::new("/h/p/shop/.agents/skills/x")
            ),
            PathBuf::from("../../.agents/skills/x")
        );
        assert_eq!(
            normalize(&Path::new("/h/p/shop/.claude/skills").join("../../.agents/skills/x")),
            PathBuf::from("/h/p/shop/.agents/skills/x")
        );
    }
}
