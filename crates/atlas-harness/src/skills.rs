//! Skill links: a harness's skills folder holds one entry per skill, and Atlas's are
//! symlinks into the roots it manages. Planning is pure: the caller reads the folder,
//! this decides, the caller applies the operations.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// What sits at `<skills folder>/<name>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Entry {
    /// A symlink whose target exists. The target is as read from the link: relative
    /// targets are resolved against the skills folder.
    Link(PathBuf),
    /// A symlink whose target is missing.
    BrokenLink(PathBuf),
    /// A real folder, such as a skill the user installed by hand.
    Dir,
    /// A regular file or anything else that isn't a link.
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillOp {
    CreateLink {
        name: String,
        target: PathBuf,
    },
    ReplaceLink {
        name: String,
        from: PathBuf,
        to: PathBuf,
    },
    RemoveLink {
        name: String,
        target: PathBuf,
    },
}

/// An entry Atlas wanted to place but won't, because it isn't Atlas's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SkillConflict {
    /// A real folder or file has the skill's name.
    NotALink { name: String },
    /// A link with the skill's name points outside Atlas's roots.
    ForeignLink { name: String, target: PathBuf },
    /// The desired source isn't inside a managed root, so Atlas couldn't recognise
    /// the link as its own later.
    SourceOutsideRoots { name: String, source: PathBuf },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SkillPlan {
    pub ops: Vec<SkillOp>,
    pub conflicts: Vec<SkillConflict>,
}

/// The operations that make `folder` hold exactly the desired Atlas skills.
///
/// Only links pointing into one of `managed_roots` are ever replaced or removed.
/// Real folders, files and links elsewhere stay; when one has a desired skill's name
/// it is reported as a conflict instead.
#[must_use]
pub fn plan_skill_links(
    folder: &Path,
    current: &BTreeMap<String, Entry>,
    desired: &BTreeMap<String, PathBuf>,
    managed_roots: &[PathBuf],
) -> SkillPlan {
    let managed = |p: &Path| managed_roots.iter().any(|r| p.starts_with(normalize(r)));
    let mut plan = SkillPlan::default();

    for (name, entry) in current {
        let target = match entry {
            Entry::Link(t) | Entry::BrokenLink(t) => normalize(&folder.join(t)),
            Entry::Dir | Entry::File => {
                if desired.contains_key(name) {
                    plan.conflicts
                        .push(SkillConflict::NotALink { name: name.clone() });
                }
                continue;
            }
        };
        if !managed(&target) {
            if desired.contains_key(name) {
                plan.conflicts.push(SkillConflict::ForeignLink {
                    name: name.clone(),
                    target,
                });
            }
            continue;
        }
        match desired.get(name).map(|s| normalize(s)) {
            Some(source) if !managed(&source) => {}
            Some(source) if source == target => {}
            Some(source) => plan.ops.push(SkillOp::ReplaceLink {
                name: name.clone(),
                from: target,
                to: source,
            }),
            None => plan.ops.push(SkillOp::RemoveLink {
                name: name.clone(),
                target,
            }),
        }
    }

    for (name, source) in desired {
        let source = normalize(source);
        if !managed(&source) {
            plan.conflicts.push(SkillConflict::SourceOutsideRoots {
                name: name.clone(),
                source,
            });
        } else if !current.contains_key(name) {
            plan.ops.push(SkillOp::CreateLink {
                name: name.clone(),
                target: source,
            });
        }
    }
    plan
}

/// Resolves `.` and `..` without touching the filesystem.
pub(crate) fn normalize(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resolves_dots() {
        assert_eq!(normalize(Path::new("/a/b/./../c")), PathBuf::from("/a/c"));
    }
}
