//! Copies of what atlasd replaces or removes when what takes its place doesn't hold the
//! same bytes, so nothing is lost (strategy, priority 1).
//!
//! A copy goes to `<replaced>/<UTC date>/<path inside the home>`, or `abs/<path>` for a
//! path outside it, before the original changes. It is written atomically (a temp file or
//! folder, then a rename), never through a link and within the `atlas_fs::library`
//! limits. A copy is never overwritten: when one with other content is already there,
//! the new one gets `.1`, `.2` … after its name; one with the same content is reused.
//! Copying it back to the original path restores it.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use atlas_fs::library::{Tree, read_tree, write_file, write_tree};

/// How many copies of one path one day can hold.
const MAX_SUFFIX: u32 = 1000;

/// Where this pass keeps its copies.
#[derive(Clone, Debug)]
pub struct Keeper {
    root: PathBuf,
    home: PathBuf,
    date: String,
}

impl Keeper {
    /// Copies go under `root` (`Paths::replaced`), in a folder named for the UTC date of
    /// `now` (seconds since the Unix epoch).
    #[must_use]
    pub fn new(root: &Path, home: &Path, now: u64) -> Self {
        Self {
            root: root.to_path_buf(),
            home: home.to_path_buf(),
            date: utc_date(now),
        }
    }

    /// The folder every copy goes under.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Keeps a copy of a file that held `bytes`. Returns where it is.
    ///
    /// # Errors
    /// The copy couldn't be written; the caller then leaves the original alone.
    pub fn file(&self, original: &Path, bytes: &[u8]) -> Result<PathBuf, String> {
        self.place(
            original,
            |at| {
                fs::symlink_metadata(at).is_ok_and(|m| m.is_file() && m.len() == bytes.len() as u64)
                    && fs::read(at).is_ok_and(|b| b == bytes)
            },
            |rel| write_file(&self.root, rel, bytes).map_err(|e| e.to_string()),
        )
    }

    /// Keeps a copy of a folder that held `tree` (read within the library's limits, with
    /// nothing in it skipped). Returns where it is.
    ///
    /// # Errors
    /// The copy couldn't be written; the caller then leaves the original alone.
    pub fn folder(&self, original: &Path, tree: &Tree) -> Result<PathBuf, String> {
        if !tree.skipped.is_empty() {
            return Err("it holds links or other things a copy can't carry".to_owned());
        }
        let digest = tree.digest();
        self.place(
            original,
            |at| {
                fs::symlink_metadata(at).is_ok_and(|m| m.is_dir())
                    && read_tree(at)
                        .ok()
                        .and_then(|t| t.exact_digest())
                        .is_some_and(|d| d == digest)
            },
            |rel| write_tree(&self.root, rel, tree).map_err(|e| e.to_string()),
        )
    }

    /// Finds the first free name for `original`'s copy (or one already holding the same
    /// content) and writes it there.
    fn place(
        &self,
        original: &Path,
        same: impl Fn(&Path) -> bool,
        write: impl Fn(&str) -> Result<(), String>,
    ) -> Result<PathBuf, String> {
        let rel = self.rel(original)?;
        self.ready_root()?;
        for n in 0..=MAX_SUFFIX {
            let candidate = if n == 0 {
                rel.clone()
            } else {
                format!("{rel}.{n}")
            };
            let at = self.root.join(&candidate);
            match fs::symlink_metadata(&at) {
                Ok(_) if same(&at) => return Ok(at),
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    write(&candidate)?;
                    return Ok(at);
                }
                Err(e) => return Err(e.to_string()),
            }
        }
        Err(format!(
            "{MAX_SUFFIX} other copies of it were already kept today"
        ))
    }

    /// The copy's path inside the root: the date, then the path inside the home, or
    /// `abs/` and the whole path.
    fn rel(&self, original: &Path) -> Result<String, String> {
        let inside = match original.strip_prefix(&self.home) {
            Ok(rest) if !rest.as_os_str().is_empty() => rest.to_path_buf(),
            _ => Path::new("abs").join(original.strip_prefix("/").unwrap_or(original)),
        };
        let rel = Path::new(&self.date).join(inside);
        rel.to_str()
            .map(str::to_owned)
            .ok_or_else(|| "its path isn't UTF-8".to_owned())
    }

    /// Creates the root as a private folder (0700) if it is missing. Refuses a link
    /// there, so copies never land somewhere else.
    fn ready_root(&self) -> Result<(), String> {
        match fs::symlink_metadata(&self.root) {
            Ok(m) if m.is_dir() => Ok(()),
            Ok(_) => Err(format!("{} isn't a folder", self.root.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir_all(&self.root)
                .and_then(|()| fs::set_permissions(&self.root, fs::Permissions::from_mode(0o700)))
                .map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// `YYYY-MM-DD` for the UTC day holding `secs` since the Unix epoch.
fn utc_date(secs: u64) -> String {
    // Howard Hinnant's `civil_from_days`, for days since 1970-01-01.
    let z = i64::try_from(secs / 86_400).unwrap_or(i64::MAX / 2) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(86_399), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_709_251_199), "2024-02-29");
        assert_eq!(utc_date(1_709_251_200), "2024-03-01");
        assert_eq!(utc_date(1_798_761_599), "2026-12-31");
    }

    #[test]
    fn copies_are_never_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let keeper = Keeper::new(&home.join("state/replaced"), &home, 0);
        let original = home.join(".codex/AGENTS.md");
        let first = keeper.file(&original, b"one").unwrap();
        assert_eq!(
            first,
            home.join("state/replaced/1970-01-01/.codex/AGENTS.md")
        );
        assert_eq!(keeper.file(&original, b"one").unwrap(), first, "reused");
        let second = keeper.file(&original, b"two").unwrap();
        assert_eq!(second, first.with_file_name("AGENTS.md.1"));
        assert_eq!(fs::read(&first).unwrap(), b"one");
        assert_eq!(fs::read(&second).unwrap(), b"two");
        assert_eq!(
            fs::metadata(home.join("state/replaced"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );

        let outside = keeper.file(Path::new("/srv/x/AGENTS.md"), b"o").unwrap();
        assert_eq!(
            outside,
            home.join("state/replaced/1970-01-01/abs/srv/x/AGENTS.md")
        );

        std::os::unix::fs::symlink(tmp.path(), home.join("state/replaced/1970-01-01/.claude"))
            .unwrap();
        assert!(
            keeper.file(&home.join(".claude/CLAUDE.md"), b"c").is_err(),
            "never through a link"
        );
        assert!(!tmp.path().join("CLAUDE.md").exists());
    }
}
