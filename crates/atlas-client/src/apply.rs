//! Carries out a plan. Each step re-checks what it is about to touch, so a folder or
//! file that appeared since planning is never replaced. A step that keeps a copy of what
//! it replaces ([`crate::keep`]) writes the copy first and, if it can't, changes nothing.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use atlas_harness::instructions::contained;

use crate::fsx;
use crate::keep::Keeper;
use crate::plan::{Action, Plan};
use crate::tree::Held;

/// What applying a plan did.
#[derive(Clone, Debug, Default)]
pub struct Applied {
    /// What was done, in words.
    pub done: Vec<String>,
    /// What failed, in words; nothing of it was changed.
    pub failed: Vec<String>,
    /// Each original kept, and where its copy is.
    pub kept: Vec<(PathBuf, PathBuf)>,
}

/// Keeps a copy of the file at `path`, which holds `bytes`.
fn keep_file(
    keeper: &Keeper,
    path: &Path,
    bytes: &[u8],
    kept: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    let copy = keeper.file(path, bytes).map_err(|e| not_kept(&e))?;
    kept.push((path.to_path_buf(), copy));
    Ok(())
}

fn not_kept(e: &str) -> io::Error {
    io::Error::other(format!(
        "no copy of it could be kept ({e}), so it was left as it is"
    ))
}

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn step(action: &Action, keeper: &Keeper, kept: &mut Vec<(PathBuf, PathBuf)>) -> io::Result<()> {
    let changed = || io::Error::other("it changed since the plan was made");
    match action {
        Action::CreateDir(dir) => fs::create_dir_all(dir),
        Action::CreateLink { path, target } => symlink(target, path),
        Action::ReplaceLink { path, to, .. } => {
            if !is_link(path) {
                return Err(changed());
            }
            fsx::replace_link(path, to)
        }
        Action::RemoveLink { path, .. } => {
            if !is_link(path) {
                return Err(changed());
            }
            fs::remove_file(path)
        }
        Action::WriteFile {
            path, bytes, keep, ..
        } => match fs::symlink_metadata(path) {
            Ok(m) if !m.is_file() => Err(changed()),
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            Ok(_) if *keep => {
                keep_file(keeper, path, &read_small_bytes(path)?, kept)?;
                fsx::write_atomic(path, bytes, None)
            }
            _ => fsx::write_atomic(path, bytes, None),
        },
        Action::RemoveFile { path, keep } => {
            if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
                return Err(changed());
            }
            if *keep {
                keep_file(keeper, path, &read_small_bytes(path)?, kept)?;
            }
            fs::remove_file(path)
        }
        Action::AdoptDir {
            path,
            target,
            digest,
            keep,
        } => adopt_dir(path, target, digest, *keep, keeper, kept),
        Action::WriteInstruction {
            path,
            was,
            bytes,
            kept_in,
            imports,
            keep,
            ..
        } => {
            if !still(path, was.as_deref())? {
                return Err(changed());
            }
            if let (Some(store), Some(was)) = (kept_in, was) {
                let mut holds = read_small(store)?;
                holds.push('\n');
                holds.push_str(&String::from_utf8_lossy(bytes));
                let atlas_import = |l: &str| imports.iter().any(|i| i == l);
                if !contained(&String::from_utf8_lossy(was), &holds, atlas_import) {
                    return Err(io::Error::other(
                        "the global instructions no longer hold all its text, so it was kept",
                    ));
                }
            }
            if let (true, Some(was)) = (*keep, was) {
                keep_file(keeper, path, was, kept)?;
            }
            fsx::write_atomic(path, bytes, None)
        }
        Action::FileToLink {
            path,
            was,
            target,
            keep,
        } => {
            if !still(path, Some(was))? {
                return Err(changed());
            }
            if !contained(&String::from_utf8_lossy(was), &read_small(target)?, |_| {
                false
            }) {
                return Err(io::Error::other(
                    "the global instructions no longer hold all its text, so it was kept",
                ));
            }
            if *keep {
                keep_file(keeper, path, was, kept)?;
            }
            // A temp link renamed over the file: never a moment without either.
            fsx::replace_link(path, target)
        }
    }
}

/// Swaps the folder at `path` for a link to `target`, only if it still holds exactly what
/// was planned and all of it is in `target`. The folder is first moved aside in one
/// rename, so nothing can change it between the check and the removal; on any doubt it
/// goes back where it was. A copy is kept first when planned (`keep`) or when `target`
/// doesn't hold it byte for byte with the same execute bits.
fn adopt_dir(
    path: &Path,
    target: &Path,
    digest: &str,
    keep: bool,
    keeper: &Keeper,
    kept: &mut Vec<(PathBuf, PathBuf)>,
) -> io::Result<()> {
    if !fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
        return Err(io::Error::other("it changed since the plan was made"));
    }
    let aside = fsx::temp_beside(path)?;
    fs::rename(path, &aside)?;
    let put_back = |e: io::Error| {
        if fs::symlink_metadata(path).is_err() {
            let _ = fs::rename(&aside, path);
        }
        e
    };
    let checked = crate::tree::exact(&aside)
        .filter(|(_, now)| now == digest)
        .map(|(tree, _)| {
            let held = crate::tree::held_by(&tree, target);
            (tree, held)
        });
    let Some((tree, held)) = checked.filter(|(_, held)| *held != Held::Not) else {
        return Err(put_back(io::Error::other(
            "it changed since the plan was made",
        )));
    };
    if keep || held != Held::Exactly {
        let copy = keeper
            .folder(path, &tree)
            .map_err(|e| put_back(not_kept(&e)))?;
        kept.push((path.to_path_buf(), copy));
    }
    symlink(target, path).map_err(put_back)?;
    // Removes the folder's own entries; links inside would not be followed, and there
    // are none (`exact` refused those).
    fs::remove_dir_all(&aside)
}

const MAX_INSTRUCTION: u64 = 1024 * 1024;

/// Whether `path` still is what the plan saw: these bytes in a regular file, or (`None`)
/// nothing at all.
fn still(path: &Path, was: Option<&[u8]>) -> io::Result<bool> {
    match (fs::symlink_metadata(path), was) {
        (Err(e), None) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        (Err(e), _) if e.kind() != io::ErrorKind::NotFound => Err(e),
        (Ok(m), Some(was)) if m.is_file() && m.len() == was.len() as u64 => {
            Ok(fs::read(path)? == was)
        }
        _ => Ok(false),
    }
}

/// A small regular file's bytes, read without following a link at its own path.
fn read_small_bytes(path: &Path) -> io::Result<Vec<u8>> {
    let m = fs::symlink_metadata(path)?;
    if !m.is_file() || m.len() > MAX_INSTRUCTION {
        return Err(io::Error::other(format!(
            "{} isn't a small regular file",
            path.display()
        )));
    }
    fs::read(path)
}

/// A small regular file's text, read without following a link at its own path.
fn read_small(path: &Path) -> io::Result<String> {
    String::from_utf8(read_small_bytes(path)?).map_err(io::Error::other)
}

/// Applies every action in order, keeping copies through `keeper`.
#[must_use]
pub fn apply(plan: &Plan, home: &Path, keeper: &Keeper) -> Applied {
    let mut applied = Applied::default();
    for action in &plan.actions {
        let said = action.describe(home, keeper.root());
        match step(action, keeper, &mut applied.kept) {
            Ok(()) => applied.done.push(said),
            Err(e) => applied.failed.push(format!("couldn't {said}: {e}")),
        }
    }
    applied
}
