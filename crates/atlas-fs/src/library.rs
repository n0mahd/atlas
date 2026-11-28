//! Folder-level changes in a store repo (creating, copying in, moving and deleting skill
//! folders) and the placements file. Bounded, never follows a link, and everything a
//! write touches stays inside the repo (cap-std). Shared by every feature that changes
//! more than one file, so there is one way to do it (decision 0027).

use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use atlas_core::catalog::{PLACEMENTS_FILE, Placements};
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};

/// Limits for a folder Atlas copies or fingerprints.
pub const MAX_FILES: usize = 1000;
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_DEPTH: usize = 16;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0} doesn't exist")]
    Missing(String),
    #[error("{0} is a link; Atlas changes folders only where they really are")]
    Symlink(String),
    #[error("the folder is too big to copy (over {MAX_FILES} files or 20 MB, or nested too deep)")]
    TooBig,
    #[error("path {0} must be relative and stay inside the repo")]
    BadPath(String),
    #[error("{PLACEMENTS_FILE} changed since it was read; reload and try again")]
    Changed,
    #[error("{PLACEMENTS_FILE} is invalid: {0}")]
    Invalid(String),
    #[error("{0}")]
    Io(String),
}

impl From<io::Error> for LibraryError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

fn check_rel(rel: &str) -> Result<&Path, LibraryError> {
    let p = Path::new(rel);
    let plain = p.components().all(|c| matches!(c, Component::Normal(_)));
    if rel.is_empty() || !plain {
        return Err(LibraryError::BadPath(rel.to_owned()));
    }
    Ok(p)
}

fn open(root: &Path) -> Result<Dir, LibraryError> {
    Ok(Dir::open_ambient_dir(root, ambient_authority())?)
}

/// One regular file of a folder, relative to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFile {
    pub rel: PathBuf,
    pub bytes: Vec<u8>,
}

/// What a folder holds, read within the limits. Links inside it are skipped and named
/// in `skipped`, because a copy must not carry a path out of the repo.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub files: Vec<TreeFile>,
    pub skipped: Vec<PathBuf>,
}

impl Tree {
    /// A fingerprint of the folder's files (names and bytes), to tell exact copies apart
    /// from edited ones.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut files: Vec<&TreeFile> = self.files.iter().collect();
        files.sort_by(|a, b| a.rel.cmp(&b.rel));
        let mut h = Sha256::new();
        for f in files {
            let name = f.rel.to_string_lossy();
            h.update((name.len() as u64).to_le_bytes());
            h.update(name.as_bytes());
            h.update((f.bytes.len() as u64).to_le_bytes());
            h.update(&f.bytes);
        }
        h.finalize().iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
    }
}

/// Reads a whole folder (an absolute path, which may itself be reached through a link,
/// as when a harness's skill entry links somewhere). Nothing below it is followed.
///
/// # Errors
/// Missing, not a folder, over the limits, or unreadable.
pub fn read_tree(folder: &Path) -> Result<Tree, LibraryError> {
    let dir = Dir::open_ambient_dir(folder, ambient_authority())
        .map_err(|_| LibraryError::Missing(folder.display().to_string()))?;
    let mut tree = Tree::default();
    let mut total = 0_u64;
    walk(&dir, Path::new(""), 0, &mut tree, &mut total)?;
    Ok(tree)
}

fn walk(
    dir: &Dir,
    at: &Path,
    depth: usize,
    tree: &mut Tree,
    total: &mut u64,
) -> Result<(), LibraryError> {
    if depth > MAX_DEPTH {
        return Err(LibraryError::TooBig);
    }
    let mut entries: Vec<_> = dir.entries()?.collect::<Result<_, _>>()?;
    entries.sort_by_key(cap_std::fs::DirEntry::file_name);
    for entry in entries {
        let name = entry.file_name();
        let rel = at.join(&name);
        let ft = entry.file_type()?;
        if ft.is_symlink() {
            tree.skipped.push(rel);
        } else if ft.is_dir() {
            if name == ".git" {
                continue;
            }
            let sub = dir.open_dir(&name)?;
            walk(&sub, &rel, depth + 1, tree, total)?;
        } else if ft.is_file() {
            if tree.files.len() >= MAX_FILES {
                return Err(LibraryError::TooBig);
            }
            let mut bytes = Vec::new();
            dir.open(&name)?
                .take(MAX_BYTES + 1 - *total)
                .read_to_end(&mut bytes)?;
            *total += bytes.len() as u64;
            if *total > MAX_BYTES {
                return Err(LibraryError::TooBig);
            }
            tree.files.push(TreeFile { rel, bytes });
        }
    }
    Ok(())
}

/// Writes `tree` as a new folder `rel` inside `root`: first into a hidden temporary
/// folder beside it, then renamed into place, so a half-written folder never appears.
///
/// # Errors
/// `rel` exists already or escapes, or a write fails (the temporary folder is removed).
pub fn write_tree(root: &Path, rel: &str, tree: &Tree) -> Result<(), LibraryError> {
    let rel_p = check_rel(rel)?;
    let dir = open(root)?;
    if dir.symlink_metadata(rel_p).is_ok() {
        return Err(LibraryError::Exists(rel.to_owned()));
    }
    let parent = rel_p.parent().unwrap_or(Path::new(""));
    let name = rel_p
        .file_name()
        .ok_or_else(|| LibraryError::BadPath(rel.to_owned()))?;
    if !parent.as_os_str().is_empty() {
        refuse_links_on_the_way(&dir, parent)?;
        dir.create_dir_all(parent)?;
    }
    let tmp = parent.join(format!(".{}.atlas-tmp", name.to_string_lossy()));
    let _ = dir.remove_dir_all(&tmp);
    let result = (|| -> Result<(), LibraryError> {
        dir.create_dir(&tmp)?;
        for f in &tree.files {
            check_rel(&f.rel.to_string_lossy())?;
            let path = tmp.join(&f.rel);
            if let Some(p) = path.parent() {
                dir.create_dir_all(p)?;
            }
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            let mut file = dir.open_with(&path, &options)?;
            file.write_all(&f.bytes)?;
            file.sync_all()?;
        }
        dir.rename(&tmp, &dir, rel_p)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = dir.remove_dir_all(&tmp);
    }
    result
}

fn refuse_links_on_the_way(dir: &Dir, rel: &Path) -> Result<(), LibraryError> {
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

/// Deletes folder `rel` inside `root` and everything in it. Refuses a link, so a
/// deletion can't reach outside the repo.
///
/// # Errors
/// Missing, a link, or the removal fails.
pub fn remove_tree(root: &Path, rel: &str) -> Result<(), LibraryError> {
    let rel_p = check_rel(rel)?;
    let dir = open(root)?;
    refuse_links_on_the_way(&dir, rel_p)?;
    let meta = dir
        .symlink_metadata(rel_p)
        .map_err(|_| LibraryError::Missing(rel.to_owned()))?;
    if !meta.is_dir() {
        return Err(LibraryError::Missing(rel.to_owned()));
    }
    dir.remove_dir_all(rel_p)?;
    Ok(())
}

/// Writes one file inside `root` atomically (temporary file, then rename), creating
/// folders on the way. Refuses links.
///
/// # Errors
/// `rel` escapes, passes through a link, or the write fails.
pub fn write_file(root: &Path, rel: &str, bytes: &[u8]) -> Result<(), LibraryError> {
    let rel_p = check_rel(rel)?;
    let dir = open(root)?;
    refuse_links_on_the_way(&dir, rel_p)?;
    if let Some(parent) = rel_p.parent().filter(|p| !p.as_os_str().is_empty()) {
        dir.create_dir_all(parent)?;
    }
    let tmp = rel_p.with_file_name(format!(
        ".{}.atlas-tmp",
        rel_p
            .file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy())
    ));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = dir.open_with(&tmp, &options)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    dir.rename(&tmp, &dir, rel_p)?;
    Ok(())
}

/// The placements and a fingerprint of the file's bytes (empty when there is no file),
/// so a later write can refuse to overwrite a change made meanwhile.
///
/// # Errors
/// The file is unreadable or not valid.
pub fn read_placements(knowledge: &Path) -> Result<(Placements, String), LibraryError> {
    let dir = open(knowledge)?;
    let text = match dir.symlink_metadata(PLACEMENTS_FILE) {
        Err(_) => return Ok((Placements::default(), String::new())),
        Ok(m) if m.file_type().is_symlink() => {
            return Err(LibraryError::Symlink(PLACEMENTS_FILE.to_owned()));
        }
        Ok(_) => {
            let mut s = String::new();
            dir.open(PLACEMENTS_FILE)?
                .take(1024 * 1024)
                .read_to_string(&mut s)?;
            s
        }
    };
    let placements = parse_placements(&text)?;
    Ok((placements, crate::edit::content_hash(&text)))
}

/// Parses `atlas/skills.toml`.
///
/// # Errors
/// Not valid TOML, or names an unknown agent or an invalid skill name.
pub fn parse_placements(text: &str) -> Result<Placements, LibraryError> {
    toml::from_str(text).map_err(|e| LibraryError::Invalid(e.message().to_owned()))
}

/// The file's text for `placements`.
#[must_use]
pub fn render_placements(placements: &Placements) -> String {
    let body = toml::to_string_pretty(placements).unwrap_or_default();
    format!(
        "# Where each skill goes. Only exceptions are listed: a skill that isn't here is on\n\
         # for every agent, everywhere. Written by Atlas; editing by hand is fine.\n\n{body}"
    )
}

/// Writes the placements if the file still has the fingerprint `expected` (from
/// [`read_placements`]). Returns the repo-relative path to commit.
///
/// # Errors
/// `Changed` when someone else wrote it meanwhile, or the write fails.
pub fn write_placements(
    knowledge: &Path,
    placements: &Placements,
    expected: &str,
) -> Result<&'static str, LibraryError> {
    let (_, now) = read_placements(knowledge)?;
    if now != expected {
        return Err(LibraryError::Changed);
    }
    write_file(
        knowledge,
        PLACEMENTS_FILE,
        render_placements(placements).as_bytes(),
    )?;
    Ok(PLACEMENTS_FILE)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use atlas_core::catalog::{Placement, SkillHome};
    use atlas_core::kernel::{HarnessKind, Slug};

    use super::*;

    #[test]
    fn a_copied_folder_matches_and_links_inside_are_skipped() {
        let src = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("scripts")).unwrap();
        fs::write(src.path().join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        fs::write(src.path().join("scripts/run.sh"), "echo hi\n").unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.path().join("leak")).unwrap();
        let tree = read_tree(src.path()).unwrap();
        assert_eq!(tree.files.len(), 2);
        assert_eq!(tree.skipped, [PathBuf::from("leak")]);

        let repo = tempfile::tempdir().unwrap();
        write_tree(repo.path(), "skills/x", &tree).unwrap();
        let copy = read_tree(&repo.path().join("skills/x")).unwrap();
        assert_eq!(copy.digest(), tree.digest());
        assert!(matches!(
            write_tree(repo.path(), "skills/x", &tree),
            Err(LibraryError::Exists(_))
        ));
        fs::write(repo.path().join("skills/x/SKILL.md"), "changed").unwrap();
        assert_ne!(
            read_tree(&repo.path().join("skills/x")).unwrap().digest(),
            tree.digest()
        );
    }

    #[test]
    fn removal_refuses_links_and_escapes() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join("skills")).unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("skills/out")).unwrap();
        assert!(matches!(
            remove_tree(repo.path(), "skills/out"),
            Err(LibraryError::Symlink(_))
        ));
        assert!(matches!(
            remove_tree(repo.path(), "../x"),
            Err(LibraryError::BadPath(_))
        ));
        assert!(outside.path().exists());
    }

    #[test]
    fn placements_round_trip_and_refuse_stale_writes() {
        let repo = tempfile::tempdir().unwrap();
        let (mut p, base) = read_placements(repo.path()).unwrap();
        assert_eq!(base, "");
        let name = Slug::new("pdf-tools").unwrap();
        p.set(
            &SkillHome::Private,
            &name,
            Placement {
                off: BTreeSet::from([HarnessKind::Codex]),
                only_projects: Some(BTreeSet::from([Slug::new("atlas").unwrap()])),
            },
        );
        write_placements(repo.path(), &p, &base).unwrap();
        let text = fs::read_to_string(repo.path().join(PLACEMENTS_FILE)).unwrap();
        assert!(text.contains("[global.pdf-tools]"), "{text}");
        assert!(text.contains("only-projects = [\"atlas\"]"), "{text}");
        let (back, now) = read_placements(repo.path()).unwrap();
        assert_eq!(back, p);
        assert!(matches!(
            write_placements(repo.path(), &p, &base),
            Err(LibraryError::Changed)
        ));
        assert!(write_placements(repo.path(), &p, &now).is_ok());
        assert!(parse_placements("[global.x]\noff = [\"nobody\"]\n").is_err());
    }
}
