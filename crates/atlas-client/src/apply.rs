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
    }
}

/// Applies every action in order. Returns the actions carried out, and what failed in
/// words.
#[must_use]
pub fn apply<'a>(plan: &'a Plan, home: &Path) -> (Vec<&'a Action>, Vec<String>) {
    let mut done = Vec::new();
    let mut failed = Vec::new();
    for action in &plan.actions {
        match step(action) {
            Ok(()) => done.push(action),
            Err(e) => failed.push(format!("couldn't {}: {e}", action.describe(home))),
        }
    }
    (done, failed)
}
