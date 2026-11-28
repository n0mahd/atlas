//! Folder-level changes in a store repo (creating, copying in, moving and deleting skill
//! folders) and the placements file. Bounded, never follows a link, never writes under
//! `.git`, and everything a write touches stays inside the repo (cap-std). Shared by every
//! feature that changes more than one file, so there is one way to do it (decision 0027).
//! `edit::write_raw` uses [`write_file`] too.

use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};

use atlas_core::catalog::{PLACEMENTS_FILE, Placements};
use cap_std::ambient_authority;
use cap_std::fs::PermissionsExt as _;
use cap_std::fs::{Dir, OpenOptions, Permissions};
use sha2::{Digest, Sha256};

/// Limits for a folder Atlas copies or fingerprints.
pub const MAX_FILES: usize = 1000;
/// Every entry counts here: files, folders, links and anything else.
pub const MAX_ENTRIES: usize = 5000;
pub const MAX_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_DEPTH: usize = 16;
/// The biggest placements file Atlas reads.
pub const MAX_PLACEMENTS_BYTES: u64 = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("{0} already exists")]
    Exists(String),
    #[error("{0} doesn't exist")]
    Missing(String),
    #[error("{0} is a link; Atlas changes folders only where they really are")]
    Symlink(String),
    #[error(
        "the folder is too big to copy (over {MAX_FILES} files, {MAX_ENTRIES} entries or 20 MB, or nested too deep)"
    )]
    TooBig,
    #[error("path {0} must be relative, stay inside the repo and not touch .git")]
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

/// A plain relative path inside a repo that never names `.git`.
fn check_rel(rel: &str) -> Result<&Path, LibraryError> {
    let p = Path::new(rel);
    let plain = p
        .components()
        .all(|c| matches!(c, Component::Normal(n) if n != ".git"));
    if rel.is_empty() || !plain {
        return Err(LibraryError::BadPath(rel.to_owned()));
    }
    Ok(p)
}

fn open(root: &Path) -> Result<Dir, LibraryError> {
    Ok(Dir::open_ambient_dir(root, ambient_authority())?)
}

/// Whether `path` is a real folder, not a link to one (the link itself is looked at).
/// The one test for "a skill folder Atlas may read or count" everywhere.
#[must_use]
pub fn is_real_dir(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

/// Whether `folder` holds a `SKILL.md` that is a regular file (not a link).
#[must_use]
pub fn has_skill_md(folder: &Path) -> bool {
    std::fs::symlink_metadata(folder.join("SKILL.md")).is_ok_and(|m| m.is_file())
}

/// Whether a link at `link` resolves to somewhere inside `within` (both looked up on
/// disk). A broken link resolves nowhere, so it doesn't.
fn resolves_inside(link: &Path, within: &Path) -> bool {
    let (Ok(target), Ok(root)) = (std::fs::canonicalize(link), std::fs::canonicalize(within))
    else {
        return false;
    };
    target.starts_with(&root)
}

/// The rule for anything Atlas installs from a repo (a skill folder, a subagent file, a
/// hook's manifest and script): a regular file, or a link that resolves inside `within`,
/// the item's own folder. `Err` says why not, for the machine's report.
///
/// # Errors
/// Missing, not a regular file, or a link that leads out of `within`.
pub fn installable_file(file: &Path, within: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(file) {
        Err(_) => Err("it is missing".to_owned()),
        Ok(m) if m.file_type().is_symlink() => {
            if resolves_inside(file, within) && std::fs::metadata(file).is_ok_and(|m| m.is_file()) {
                Ok(())
            } else {
                Err("it is a link to somewhere else".to_owned())
            }
        }
        Ok(m) if m.is_file() => Ok(()),
        Ok(_) => Err("it isn't a regular file".to_owned()),
    }
}

/// The same rule for a whole folder Atlas links agents to (a skill): a real folder whose
/// `required` file passes [`installable_file`], and no link anywhere inside it that leads
/// out of the folder, since agents read through the folder's links. Bounded like
/// [`read_tree`].
///
/// # Errors
/// The first reason the folder can't be installed, naming the entry.
pub fn installable_folder(folder: &Path, required: &str) -> Result<(), String> {
    if !is_real_dir(folder) {
        return Err("it is a link to somewhere else".to_owned());
    }
    installable_file(&folder.join(required), folder)
        .map_err(|why| format!("its {required}: {why}"))?;
    if let Some(rel) = links_in(folder)?
        .into_iter()
        .find(|rel| !resolves_inside(&folder.join(rel), folder))
    {
        return Err(format!(
            "{} inside it is a link to somewhere else",
            rel.display()
        ));
    }
    Ok(())
}

/// Every link under `folder` (none followed), relative to it, in name order; bounded
/// like [`read_tree`]. Items whose content must be exactly what an update shows (hooks)
/// refuse any of them; skills refuse those that lead out ([`installable_folder`]).
///
/// # Errors
/// The folder can't be listed, or holds too many entries.
pub fn links_in(folder: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    let mut seen = 0_usize;
    while let Some(rel) = stack.pop() {
        let entries: Vec<_> = std::fs::read_dir(folder.join(&rel))
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        for entry in entries {
            seen += 1;
            if seen > MAX_ENTRIES {
                return Err("it holds too many files".to_owned());
            }
            let path = rel.join(entry.file_name());
            let ft = entry.file_type().map_err(|e| e.to_string())?;
            if ft.is_symlink() {
                out.push(path);
            } else if ft.is_dir() && entry.file_name() != ".git" {
                stack.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// One regular file of a folder, relative to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeFile {
    pub rel: PathBuf,
    pub bytes: Vec<u8>,
    /// Whether any execute bit is set (a skill's helper scripts).
    pub executable: bool,
}

/// What a folder holds, read within the limits. Anything that isn't a plain file or
/// folder (links, a `.git` folder, sockets) is left out and named in `skipped`, because a
/// copy must not carry a path out of the repo or someone's repository history.
#[derive(Clone, Debug, Default)]
pub struct Tree {
    pub files: Vec<TreeFile>,
    pub skipped: Vec<PathBuf>,
}

impl Tree {
    /// A fingerprint of the folder's files (names, bytes and execute bits). It ignores
    /// what is in `skipped`, so use [`Tree::exact_digest`] to decide that two folders are
    /// the same.
    #[must_use]
    pub fn digest(&self) -> String {
        let mut files: Vec<&TreeFile> = self.files.iter().collect();
        files.sort_by(|a, b| a.rel.cmp(&b.rel));
        let mut h = Sha256::new();
        for f in files {
            let name = f.rel.to_string_lossy();
            h.update((name.len() as u64).to_le_bytes());
            h.update(name.as_bytes());
            h.update([u8::from(f.executable)]);
            h.update((f.bytes.len() as u64).to_le_bytes());
            h.update(&f.bytes);
        }
        h.finalize().iter().fold(String::new(), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    /// The fingerprint only when the folder holds nothing but plain files and folders.
    /// A folder with a link, a `.git` or anything else Atlas would not copy never counts
    /// as an exact copy, so atlasd never replaces it (nothing is lost).
    #[must_use]
    pub fn exact_digest(&self) -> Option<String> {
        self.skipped.is_empty().then(|| self.digest())
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
    let mut walk = Walk::default();
    walk.dir(&dir, Path::new(""), 0)?;
    Ok(walk.tree)
}

#[derive(Default)]
struct Walk {
    tree: Tree,
    bytes: u64,
    entries: usize,
}

impl Walk {
    fn dir(&mut self, dir: &Dir, at: &Path, depth: usize) -> Result<(), LibraryError> {
        if depth > MAX_DEPTH {
            return Err(LibraryError::TooBig);
        }
        let mut entries: Vec<_> = dir.entries()?.collect::<Result<_, _>>()?;
        self.entries += entries.len();
        if self.entries > MAX_ENTRIES {
            return Err(LibraryError::TooBig);
        }
        entries.sort_by_key(cap_std::fs::DirEntry::file_name);
        for entry in entries {
            let name = entry.file_name();
            let rel = at.join(&name);
            let ft = entry.file_type()?;
            if ft.is_dir() && name != ".git" {
                let sub = dir.open_dir(&name)?;
                self.dir(&sub, &rel, depth + 1)?;
            } else if ft.is_file() {
                if self.tree.files.len() >= MAX_FILES {
                    return Err(LibraryError::TooBig);
                }
                let file = dir.open(&name)?;
                let executable = file.metadata()?.permissions().mode() & 0o111 != 0;
                let mut bytes = Vec::new();
                file.take(MAX_BYTES + 1 - self.bytes)
                    .read_to_end(&mut bytes)?;
                self.bytes += bytes.len() as u64;
                if self.bytes > MAX_BYTES {
                    return Err(LibraryError::TooBig);
                }
                self.tree.files.push(TreeFile {
                    rel,
                    bytes,
                    executable,
                });
            } else {
                // Links, `.git` and special files.
                self.tree.skipped.push(rel);
            }
        }
        Ok(())
    }
}

/// Writes `tree` as a new folder `rel` inside `root`: first into a hidden temporary
/// folder beside it, then renamed into place, so a half-written folder never appears.
/// Execute bits are kept. Durability comes from the commit that follows.
///
/// # Errors
/// `rel` exists already or escapes, or a write fails (the temporary folder is removed).
pub fn write_tree(root: &Path, rel: &str, tree: &Tree) -> Result<(), LibraryError> {
    let rel_p = check_rel(rel)?;
    let dir = open(root)?;
    refuse_links_on_the_way(&dir, rel_p)?;
    if dir.symlink_metadata(rel_p).is_ok() {
        return Err(LibraryError::Exists(rel.to_owned()));
    }
    let parent = rel_p.parent().unwrap_or(Path::new(""));
    let name = rel_p
        .file_name()
        .ok_or_else(|| LibraryError::BadPath(rel.to_owned()))?;
    if !parent.as_os_str().is_empty() {
        dir.create_dir_all(parent)?;
    }
    let tmp = parent.join(format!(".{}.atlas-tmp", name.to_string_lossy()));
    remove_stale(&dir, &tmp)?;
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
            if f.executable {
                file.set_permissions(Permissions::from_mode(0o755))?;
            }
        }
        dir.rename(&tmp, &dir, rel_p)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = dir.remove_dir_all(&tmp);
    }
    result
}

/// Removes a temporary file or folder left by an earlier failed write, never through a
/// link (a link with that name is removed itself).
fn remove_stale(dir: &Dir, tmp: &Path) -> Result<(), LibraryError> {
    match dir.symlink_metadata(tmp) {
        Ok(m) if m.is_dir() => dir.remove_dir_all(tmp)?,
        Ok(_) => dir.remove_file(tmp)?,
        Err(_) => {}
    }
    Ok(())
}

/// Fails if any component of `rel` (the last one included) is a link.
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

/// Writes one file inside `root` atomically (a fresh temporary file beside it, then a
/// rename), creating folders on the way. Refuses links anywhere on the path and `.git`.
/// The one atomic write in `atlas-fs`.
///
/// # Errors
/// `rel` escapes, passes through a link, or the write fails (the temporary file is
/// removed).
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
    remove_stale(&dir, &tmp)?;
    let result = (|| -> Result<(), LibraryError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = dir.open_with(&tmp, &options)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        dir.rename(&tmp, &dir, rel_p)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = dir.remove_file(&tmp);
    }
    result
}

/// The placements and a fingerprint of the file's bytes (empty when there is no file),
/// so a later write can refuse to overwrite a change made meanwhile.
///
/// # Errors
/// The file (or its folder) is a link, too big, unreadable or not valid. Only a missing
/// file means "no exceptions".
pub fn read_placements(knowledge: &Path) -> Result<(Placements, String), LibraryError> {
    let dir = open(knowledge)?;
    refuse_links_on_the_way(&dir, Path::new(PLACEMENTS_FILE))?;
    match dir.symlink_metadata(PLACEMENTS_FILE) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((Placements::default(), String::new()));
        }
        Err(e) => return Err(e.into()),
        Ok(m) if !m.is_file() => {
            return Err(LibraryError::Invalid("it isn't a regular file".to_owned()));
        }
        Ok(_) => {}
    }
    let mut text = String::new();
    dir.open(PLACEMENTS_FILE)?
        .take(MAX_PLACEMENTS_BYTES + 1)
        .read_to_string(&mut text)?;
    if text.len() as u64 > MAX_PLACEMENTS_BYTES {
        return Err(LibraryError::Invalid("it is larger than 1 MiB".to_owned()));
    }
    let placements = parse_placements(&text)?;
    Ok((placements, crate::edit::content_hash(&text)))
}

/// Parses `atlas/placements.toml`.
///
/// # Errors
/// Not valid TOML, or names an unknown agent or an invalid skill name.
pub fn parse_placements(text: &str) -> Result<Placements, LibraryError> {
    toml::from_str(text).map_err(|e| LibraryError::Invalid(e.message().to_owned()))
}

/// The file's text for `placements`.
///
/// # Errors
/// The placements can't be written as TOML (never silently an empty file).
pub fn render_placements(placements: &Placements) -> Result<String, LibraryError> {
    let body =
        toml::to_string_pretty(placements).map_err(|e| LibraryError::Invalid(e.to_string()))?;
    Ok(format!(
        "# Where each skill goes. Only exceptions are listed: a skill that isn't here is on\n\
         # for every agent, everywhere. Atlas rewrites this file, so comments aren't kept.\n\n{body}"
    ))
}

/// Writes the placements if the file still has the fingerprint `expected` (from
/// [`read_placements`]). Writes nothing when there is no file and nothing to say.
/// Returns the repo-relative path to commit.
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
    if now.is_empty() && *placements == Placements::default() {
        return Ok(PLACEMENTS_FILE);
    }
    write_file(
        knowledge,
        PLACEMENTS_FILE,
        render_placements(placements)?.as_bytes(),
    )?;
    Ok(PLACEMENTS_FILE)
}

/// Adds `patterns` (such as `/CLAUDE.local.md`) to Atlas's block in `<root>/.git/info/exclude`
/// (`atlas_harness::instructions::exclude_with`), keeping every other line. The only write
/// Atlas makes inside `.git`: it touches nothing else there, and refuses a `.git` that is a
/// file (a worktree) or a link anywhere on the way. Returns whether the file changed.
///
/// # Errors
/// No `.git` folder, a link on the way, an exclude file over 1 MiB, or the write fails.
pub fn add_to_git_exclude(root: &Path, patterns: &[&str]) -> Result<bool, LibraryError> {
    const MAX_EXCLUDE: u64 = 1024 * 1024;
    let dir = open(root)?;
    match dir.symlink_metadata(".git") {
        Ok(m) if m.is_dir() => {}
        Ok(m) if m.file_type().is_symlink() => return Err(LibraryError::Symlink(".git".into())),
        _ => {
            return Err(LibraryError::Io(
                "this checkout has no .git folder (a worktree keeps it elsewhere)".to_owned(),
            ));
        }
    }
    let git = dir.open_dir(".git")?;
    match git.symlink_metadata("info") {
        Ok(m) if m.is_dir() => {}
        Ok(_) => return Err(LibraryError::Symlink(".git/info".into())),
        Err(_) => git.create_dir("info")?,
    }
    let info = git.open_dir("info")?;
    let current = match info.symlink_metadata("exclude") {
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
        Ok(m) if !m.is_file() => return Err(LibraryError::Symlink(".git/info/exclude".into())),
        Ok(_) => {
            let mut text = String::new();
            info.open("exclude")?
                .take(MAX_EXCLUDE + 1)
                .read_to_string(&mut text)?;
            if text.len() as u64 > MAX_EXCLUDE {
                return Err(LibraryError::TooBig);
            }
            text
        }
    };
    let next = atlas_harness::instructions::exclude_with(&current, patterns);
    if next == current {
        return Ok(false);
    }
    let tmp = Path::new(".exclude.atlas-tmp");
    remove_stale(&info, tmp)?;
    let result = (|| -> Result<(), LibraryError> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        let mut file = info.open_with(tmp, &options)?;
        file.write_all(next.as_bytes())?;
        file.sync_all()?;
        drop(file);
        info.rename(tmp, &info, "exclude")?;
        Ok(())
    })();
    if result.is_err() {
        let _ = info.remove_file(tmp);
    }
    result.map(|()| true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::os::unix::fs::PermissionsExt as _;

    use atlas_core::catalog::{Placement, SkillHome};
    use atlas_core::kernel::{HarnessKind, Slug};

    use super::*;

    #[test]
    fn a_copied_folder_matches_and_keeps_execute_bits() {
        let src = tempfile::tempdir().unwrap();
        fs::create_dir_all(src.path().join("scripts")).unwrap();
        fs::write(src.path().join("SKILL.md"), "---\nname: x\n---\n").unwrap();
        fs::write(src.path().join("scripts/run.sh"), "echo hi\n").unwrap();
        fs::set_permissions(
            src.path().join("scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let tree = read_tree(src.path()).unwrap();
        assert_eq!(tree.files.len(), 2);
        assert!(tree.exact_digest().is_some());

        let repo = tempfile::tempdir().unwrap();
        write_tree(repo.path(), "skills/x", &tree).unwrap();
        let mode = fs::metadata(repo.path().join("skills/x/scripts/run.sh"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "execute bit kept");
        let copy = read_tree(&repo.path().join("skills/x")).unwrap();
        assert_eq!(copy.exact_digest(), tree.exact_digest());
        assert!(matches!(
            write_tree(repo.path(), "skills/x", &tree),
            Err(LibraryError::Exists(_))
        ));
        fs::set_permissions(
            repo.path().join("skills/x/scripts/run.sh"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_ne!(
            read_tree(&repo.path().join("skills/x")).unwrap().digest(),
            tree.digest(),
            "a lost execute bit is a difference"
        );
    }

    #[test]
    fn links_and_git_folders_are_never_an_exact_copy() {
        let src = tempfile::tempdir().unwrap();
        fs::write(src.path().join("SKILL.md"), "x\n").unwrap();
        let plain = read_tree(src.path()).unwrap();
        std::os::unix::fs::symlink("/etc/passwd", src.path().join("leak")).unwrap();
        fs::create_dir_all(src.path().join(".git/objects")).unwrap();
        let tree = read_tree(src.path()).unwrap();
        assert_eq!(tree.files.len(), 1);
        assert_eq!(tree.skipped, [PathBuf::from(".git"), PathBuf::from("leak")]);
        // Same files, so `digest` matches, but the folder holds more than Atlas would copy.
        assert_eq!(tree.digest(), plain.digest());
        assert_eq!(tree.exact_digest(), None);
    }

    #[test]
    fn too_many_entries_is_refused() {
        let src = tempfile::tempdir().unwrap();
        for i in 0..=MAX_ENTRIES {
            std::os::unix::fs::symlink("/nowhere", src.path().join(format!("l{i}"))).unwrap();
        }
        assert!(matches!(read_tree(src.path()), Err(LibraryError::TooBig)));
    }

    #[test]
    fn removal_and_writes_refuse_links_escapes_and_git() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::create_dir_all(repo.path().join("skills")).unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("skills/out")).unwrap();
        assert!(matches!(
            remove_tree(repo.path(), "skills/out"),
            Err(LibraryError::Symlink(_))
        ));
        assert!(matches!(
            write_file(repo.path(), "skills/out/x.md", b"x"),
            Err(LibraryError::Symlink(_))
        ));
        assert!(matches!(
            remove_tree(repo.path(), "../x"),
            Err(LibraryError::BadPath(_))
        ));
        assert!(matches!(
            write_file(repo.path(), ".git/config", b"x"),
            Err(LibraryError::BadPath(_))
        ));
        assert!(outside.path().read_dir().unwrap().next().is_none());
    }

    #[test]
    fn a_planted_temporary_link_is_not_followed() {
        let repo = tempfile::tempdir().unwrap();
        fs::write(repo.path().join("victim.md"), "keep\n").unwrap();
        std::os::unix::fs::symlink(
            repo.path().join("victim.md"),
            repo.path().join(".notes.md.atlas-tmp"),
        )
        .unwrap();
        write_file(repo.path(), "notes.md", b"new\n").unwrap();
        assert_eq!(
            fs::read_to_string(repo.path().join("victim.md")).unwrap(),
            "keep\n"
        );
        assert_eq!(
            fs::read_to_string(repo.path().join("notes.md")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn the_exclude_block_is_the_only_write_inside_git() {
        let repo = tempfile::tempdir().unwrap();
        assert!(
            add_to_git_exclude(repo.path(), &["/CLAUDE.local.md"]).is_err(),
            "no .git"
        );
        fs::create_dir_all(repo.path().join(".git/info")).unwrap();
        fs::write(repo.path().join(".git/info/exclude"), "*.swp\n").unwrap();
        assert!(add_to_git_exclude(repo.path(), &["/CLAUDE.local.md"]).unwrap());
        assert!(!add_to_git_exclude(repo.path(), &["/CLAUDE.local.md"]).unwrap());
        assert_eq!(
            fs::read_to_string(repo.path().join(".git/info/exclude")).unwrap(),
            "*.swp\n# atlas:begin\n/CLAUDE.local.md\n# atlas:end\n"
        );
        let worktree = tempfile::tempdir().unwrap();
        fs::write(worktree.path().join(".git"), "gitdir: /elsewhere\n").unwrap();
        assert!(add_to_git_exclude(worktree.path(), &["/x"]).is_err());
    }

    #[test]
    fn placements_round_trip_and_refuse_stale_writes() {
        let repo = tempfile::tempdir().unwrap();
        let (mut p, base) = read_placements(repo.path()).unwrap();
        assert_eq!(base, "");
        write_placements(repo.path(), &p, &base).unwrap();
        assert!(
            !repo.path().join(PLACEMENTS_FILE).exists(),
            "nothing to say, no file"
        );
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

    #[test]
    fn placements_errors_are_not_mistaken_for_no_file() {
        let repo = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("placements.toml"), "").unwrap();
        std::os::unix::fs::symlink(outside.path(), repo.path().join("atlas")).unwrap();
        assert!(matches!(
            read_placements(repo.path()),
            Err(LibraryError::Symlink(_))
        ));
        let big = tempfile::tempdir().unwrap();
        fs::create_dir_all(big.path().join("atlas")).unwrap();
        let line = "# padding\n".repeat(110_000);
        fs::write(big.path().join(PLACEMENTS_FILE), line).unwrap();
        assert!(matches!(
            read_placements(big.path()),
            Err(LibraryError::Invalid(_))
        ));
    }
}
