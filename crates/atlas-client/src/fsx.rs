//! Small filesystem moves that never leave a half-written file or a missing link.

use std::fs;
use std::io::{self, Write as _};
use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _, symlink};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A fresh hidden name next to `path`, for a temp file, link or folder.
pub fn temp_beside(path: &Path) -> io::Result<PathBuf> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no folder"))?;
    let name = path
        .file_name()
        .ok_or_else(|| io::Error::other("path has no name"))?
        .to_string_lossy();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(parent.join(format!(".{name}.atlas-{}-{n}", std::process::id())))
}

/// Writes `bytes` to `path` through a temp file in the same folder and a rename.
/// `mode` sets the permissions; None keeps an existing file's mode, or the umask's
/// default for a new file. Never writes through a link: the rename replaces the entry.
pub fn write_atomic(path: &Path, bytes: &[u8], mode: Option<u32>) -> io::Result<()> {
    let keep = fs::symlink_metadata(path)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|m| m.permissions().mode() & 0o7777);
    let tmp = temp_beside(path)?;
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(mode.unwrap_or(0o666))
            .open(&tmp)?;
        file.write_all(bytes)?;
        if let Some(m) = mode.or(keep) {
            file.set_permissions(fs::Permissions::from_mode(m))?;
        }
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// A private file (0600) in a private folder (0700), for the client's config and state.
pub fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("path has no folder"))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    write_atomic(path, bytes, Some(0o600))
}

/// Points the link at `path` to `target` in one step: a temp link, then a rename over
/// the old one. The caller has checked that `path` is a link, or a file it means to
/// replace with one.
pub fn replace_link(path: &Path, target: &Path) -> io::Result<()> {
    let tmp = temp_beside(path)?;
    symlink(target, &tmp)?;
    fs::rename(&tmp, path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })
}
