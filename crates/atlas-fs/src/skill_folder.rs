//! A skill's folder as a whole: moving it between repos, reading one of its files for
//! the viewer, and fingerprints of same-named copies so the list can mark conflicts.
//! Built on [`crate::library`]; never follows a link inside a repo.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use atlas_core::catalog::{Catalog, SkillHome};
use atlas_core::distribution::HarnessView;
use cap_std::ambient_authority;
use cap_std::fs::Dir;

use crate::Sources;
use crate::library::{self, LibraryError};

/// The repo a skill's home is in, and the folder's path inside it.
#[must_use]
pub fn locate(sources: &Sources, home: &SkillHome, name: &str) -> Option<(PathBuf, String)> {
    match home {
        SkillHome::Private => Some((sources.knowledge.clone(), format!("skills/{name}"))),
        SkillHome::Public => Some((sources.public_skills.clone()?, name.to_owned())),
        SkillHome::Project(p) => {
            let project = sources.projects.iter().find(|x| &x.name == p)?;
            Some((project.root.clone(), format!(".agents/skills/{name}")))
        }
    }
}

fn plain(rel: &str) -> Option<&Path> {
    let p = Path::new(rel);
    (!rel.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))).then_some(p)
}

fn refuse_links(dir: &Dir, rel: &Path) -> Result<(), LibraryError> {
    let mut at = PathBuf::new();
    for c in rel.components() {
        at.push(c);
        if dir
            .symlink_metadata(&at)
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(LibraryError::Symlink(at.display().to_string()));
        }
    }
    Ok(())
}

/// Moves folder `from_rel` of `from_root` to `to_rel` of `to_root` (which must not exist):
/// copies it whole into place (execute bits kept), then removes the original, so a
/// failure part way leaves an extra copy, never none. A folder holding anything a copy
/// can't carry (a link, a `.git`, a socket) is refused, so nothing is lost.
///
/// # Errors
/// The source is missing, is or holds a link, the destination exists, or a write fails.
pub fn move_folder(
    from_root: &Path,
    from_rel: &str,
    to_root: &Path,
    to_rel: &str,
) -> Result<(), LibraryError> {
    let from_p = plain(from_rel).ok_or_else(|| LibraryError::BadPath(from_rel.to_owned()))?;
    let from_dir = Dir::open_ambient_dir(from_root, ambient_authority())?;
    refuse_links(&from_dir, from_p)?;
    let tree = library::read_tree(&from_root.join(from_p))?;
    if let Some(odd) = tree.skipped.first() {
        return Err(LibraryError::Io(format!(
            "{from_rel}/{} can't be copied (a link, a .git folder or a special file), so move this skill by hand",
            odd.display()
        )));
    }
    library::write_tree(to_root, to_rel, &tree)?;
    library::remove_tree(from_root, from_rel)
}

/// Why a skill file can't be shown.
#[derive(Debug, PartialEq, Eq)]
pub enum ViewError {
    Missing,
    Link,
    TooBig,
}

/// Reads `file` of the skill folder `folder_rel` inside `root`, at most `max` bytes.
/// Refuses links anywhere on the way and anything that isn't a regular file.
///
/// # Errors
/// See [`ViewError`].
pub fn read_file(
    root: &Path,
    folder_rel: &str,
    file: &str,
    max: u64,
) -> Result<Vec<u8>, ViewError> {
    let folder = plain(folder_rel).ok_or(ViewError::Missing)?;
    let file = plain(file).ok_or(ViewError::Missing)?;
    let rel = folder.join(file);
    let dir = Dir::open_ambient_dir(root, ambient_authority()).map_err(|_| ViewError::Missing)?;
    refuse_links(&dir, &rel).map_err(|_| ViewError::Link)?;
    let meta = dir.symlink_metadata(&rel).map_err(|_| ViewError::Missing)?;
    if !meta.is_file() {
        return Err(ViewError::Missing);
    }
    if meta.len() > max {
        return Err(ViewError::TooBig);
    }
    let mut bytes = Vec::new();
    dir.open(&rel)
        .and_then(|f| f.take(max + 1).read_to_end(&mut bytes))
        .map_err(|_| ViewError::Missing)?;
    if bytes.len() as u64 > max {
        return Err(ViewError::TooBig);
    }
    Ok(bytes)
}

/// One copy of a skill name that agents can read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Copy {
    /// Where it is, as shown (`~/...`).
    pub shown: String,
    /// The folder's exact fingerprint ([`library::Tree::exact_digest`]); None when it
    /// can't be read or holds links or other things a copy can't carry.
    pub digest: Option<String>,
    /// It is a skill in the store (not a harness's own folder).
    pub in_store: bool,
}

fn digest_of(folder: &Path) -> Option<String> {
    library::read_tree(folder).ok()?.exact_digest()
}

/// Every name that more than one place holds a real copy of (store homes, and harness
/// folders' own folders or outside links), with a fingerprint of each copy's folder.
/// Reads only the folders of names that occur twice or more, so it stays cheap.
#[must_use]
pub fn same_named(
    sources: &Sources,
    catalog: &Catalog,
    harnesses: &[HarnessView],
) -> BTreeMap<String, Vec<Copy>> {
    use atlas_core::distribution::InstalledSkill;
    // name -> (shown, absolute SKILL.md, in store)
    let mut places: BTreeMap<String, Vec<(String, PathBuf, bool)>> = BTreeMap::new();
    for s in &catalog.skills {
        if let Some((root, rel)) = locate(sources, &s.home, s.name.as_str()) {
            let md = root.join(&rel).join("SKILL.md");
            places.entry(s.name.to_string()).or_default().push((
                sources.display(&root.join(rel)),
                md,
                true,
            ));
        }
    }
    let layouts = atlas_harness::layouts();
    for h in harnesses {
        let Some(layout) = layouts.iter().find(|l| l.kind == h.kind) else {
            continue;
        };
        for (dir, rel_dir) in h.skill_dirs.iter().zip(layout.skill_dirs) {
            for e in &dir.entries {
                let InstalledSkill::Outside { name, .. } = e else {
                    continue;
                };
                let folder = sources.home.join(rel_dir).join(name);
                let shown = sources.display(&folder);
                let list = places.entry(name.clone()).or_default();
                // Two harnesses reading one folder hold one copy.
                if !list.iter().any(|(s, ..)| *s == shown) {
                    list.push((shown, folder, false));
                }
            }
        }
    }
    places
        .into_iter()
        .filter(|(_, v)| v.len() > 1)
        .map(|(name, v)| {
            let copies = v
                .into_iter()
                .map(|(shown, folder, in_store)| Copy {
                    shown,
                    digest: digest_of(&folder),
                    in_store,
                })
                .collect();
            (name, copies)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[test]
    fn a_move_keeps_scripts_runnable_and_refuses_links() {
        use std::os::unix::fs::PermissionsExt as _;
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        fs::create_dir_all(a.path().join("skills/x/scripts")).unwrap();
        fs::write(a.path().join("skills/x/SKILL.md"), "x").unwrap();
        fs::write(a.path().join("skills/x/scripts/run.sh"), "echo").unwrap();
        fs::set_permissions(
            a.path().join("skills/x/scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        move_folder(a.path(), "skills/x", b.path(), ".agents/skills/x").unwrap();
        assert_eq!(
            fs::read(b.path().join(".agents/skills/x/SKILL.md")).unwrap(),
            b"x"
        );
        assert!(!a.path().join("skills/x").exists());
        let mode = fs::metadata(b.path().join(".agents/skills/x/scripts/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o111, 0o111);

        fs::create_dir_all(a.path().join("skills/y")).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", a.path().join("skills/y/leak")).unwrap();
        assert!(move_folder(a.path(), "skills/y", b.path(), "y").is_err());
        assert!(a.path().join("skills/y").exists() && !b.path().join("y").exists());
    }

    #[test]
    fn the_viewer_reads_only_plain_files_inside() {
        let a = tempfile::tempdir().unwrap();
        fs::create_dir_all(a.path().join("skills/x")).unwrap();
        fs::write(a.path().join("skills/x/notes.md"), "hello").unwrap();
        fs::write(a.path().join("secret"), "no").unwrap();
        std::os::unix::fs::symlink("../../secret", a.path().join("skills/x/link")).unwrap();
        assert_eq!(
            read_file(a.path(), "skills/x", "notes.md", 100).unwrap(),
            b"hello"
        );
        assert_eq!(
            read_file(a.path(), "skills/x", "link", 100),
            Err(ViewError::Link)
        );
        assert_eq!(
            read_file(a.path(), "skills/x", "../../secret", 100),
            Err(ViewError::Missing)
        );
        assert_eq!(
            read_file(a.path(), "skills/x", "notes.md", 2),
            Err(ViewError::TooBig)
        );
    }
}
