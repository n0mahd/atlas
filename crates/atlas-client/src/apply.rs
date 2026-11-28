//! Carries out a plan. Each step re-checks what it is about to touch, so a folder or
//! file that appeared since planning is never replaced.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::Path;

use crate::fsx;
use crate::plan::{Action, Plan};

fn is_link(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

fn step(action: &Action) -> io::Result<()> {
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
        Action::WriteFile { path, bytes, .. } => match fs::symlink_metadata(path) {
            Ok(m) if !m.is_file() => Err(changed()),
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => fsx::write_atomic(path, bytes, None),
        },
        Action::RemoveFile { path } => {
            if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file()) {
                return Err(changed());
            }
            fs::remove_file(path)
        }
        Action::AdoptDir {
            path,
            target,
            digest,
        } => adopt_dir(path, target, digest),
    }
}

/// Swaps the folder at `path` for a link to `target`, only if it still holds exactly what
/// was planned and all of it is in `target`. The folder is first moved aside in one
/// rename, so nothing can change it between the check and the removal; on any doubt it
/// goes back where it was.
fn adopt_dir(path: &Path, target: &Path, digest: &str) -> io::Result<()> {
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
    let same = crate::tree::exact(&aside)
        .is_some_and(|(tree, now)| now == digest && crate::tree::held_by(&tree, target));
    if !same {
        return Err(put_back(io::Error::other(
            "it changed since the plan was made",
        )));
    }
    symlink(target, path).map_err(put_back)?;
    // Removes the folder's own entries; links inside would not be followed, and there
    // are none (`exact` refused those).
    fs::remove_dir_all(&aside)
}

/// Applies every action in order. Returns what was done and what failed, in words.
#[must_use]
pub fn apply(plan: &Plan, home: &Path) -> (Vec<String>, Vec<String>) {
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for action in &plan.actions {
        match step(action) {
            Ok(()) => done.push(action.describe(home)),
            Err(e) => failed.push(format!("couldn't {}: {e}", action.describe(home))),
        }
    }
    (done, failed)
}
