//! Local copies of a release's sources, at `<sources>/<key>`, each checked out at the
//! pinned commit only once its tree matches the release. The git remotes are untrusted
//! storage; the pinned tree is what counts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use atlas_core::release::{Manifest, Source, SourceId};
use atlas_git::Repo;

/// The folder name of a source's local copy.
#[must_use]
pub fn key(id: &SourceId) -> String {
    match id {
        SourceId::Knowledge => "knowledge".to_owned(),
        SourceId::PublicSkills => "public-skills".to_owned(),
        SourceId::Project(slug) => format!("project-{slug}"),
    }
}

fn label(id: &SourceId) -> String {
    match id {
        SourceId::Knowledge => "the knowledge repo".to_owned(),
        SourceId::PublicSkills => "the public skills repo".to_owned(),
        SourceId::Project(slug) => format!("project {slug}"),
    }
}

/// Skills, instructions and hooks come from these; the others are fetched for later.
fn feeds_home(id: &SourceId) -> bool {
    matches!(id, SourceId::Knowledge | SourceId::PublicSkills)
}

/// The local copies that are ready to use, and what went wrong with the others.
#[derive(Clone, Debug, Default)]
pub struct Copies {
    pub dirs: BTreeMap<SourceId, PathBuf>,
    pub problems: Vec<String>,
}

fn fetch_and_check(source: &Source, dir: &Path) -> Result<Repo, String> {
    let fail = |e: atlas_git::GitError| format!("couldn't fetch {}: {e}", label(&source.id));
    let repo = Repo::clone_or_open(&source.remote, dir).map_err(fail)?;
    repo.fetch_commit(source.commit.as_str()).map_err(fail)?;
    let tree = repo.tree_of(source.commit.as_str()).map_err(fail)?;
    if tree != source.tree.as_str() {
        return Err(format!(
            "{} at commit {} has tree {tree}, but the update pins {}; not using it",
            label(&source.id),
            source.commit.as_str(),
            source.tree
        ));
    }
    Ok(repo)
}

/// Fetches every source and checks its tree before checking any of them out, so a
/// release whose knowledge or public skills don't match leaves every copy as it was.
///
/// # Errors
/// The knowledge or public skills source couldn't be fetched or doesn't match: the
/// whole release is refused. A project source that fails is only a problem.
pub fn sync(manifest: &Manifest, sources_dir: &Path) -> Result<Copies, String> {
    let mut copies = Copies::default();
    let mut ready = Vec::new();
    for source in &manifest.sources {
        let dir = sources_dir.join(key(&source.id));
        match fetch_and_check(source, &dir) {
            Ok(repo) => ready.push((source, repo, dir)),
            Err(e) if feeds_home(&source.id) => return Err(e),
            Err(e) => copies.problems.push(e),
        }
    }
    for (source, repo, dir) in ready {
        match repo.checkout_detached(source.commit.as_str()) {
            Ok(()) => {
                copies.dirs.insert(source.id.clone(), dir);
            }
            Err(e) => {
                let msg = format!("couldn't check out {}: {e}", label(&source.id));
                if feeds_home(&source.id) {
                    return Err(msg);
                }
                copies.problems.push(msg);
            }
        }
    }
    Ok(copies)
}

/// For a dry run: the copies as they stand, without fetching. Notes each copy that
/// isn't at the release's commit yet.
#[must_use]
pub fn existing(manifest: &Manifest, sources_dir: &Path) -> (Copies, Vec<String>) {
    let mut copies = Copies::default();
    let mut notes = Vec::new();
    for source in &manifest.sources {
        let dir = sources_dir.join(key(&source.id));
        let Ok(repo) = Repo::open(&dir) else {
            notes.push(format!(
                "{} has no local copy yet; a real run fetches it first",
                label(&source.id)
            ));
            continue;
        };
        if repo.head().map(|(c, _)| c).ok().as_deref() != Some(source.commit.as_str()) {
            notes.push(format!(
                "the local copy of {} isn't at this update's commit yet; this plan uses it as it stands",
                label(&source.id)
            ));
        }
        copies.dirs.insert(source.id.clone(), dir);
    }
    (copies, notes)
}
