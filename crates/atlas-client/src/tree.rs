//! Fingerprints of real folders, to tell an exact copy of what Atlas holds (which atlasd
//! may replace with its link) from anything else (which it never touches). Bounded, and
//! never follows a link.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_fs::library::{MAX_DEPTH, MAX_FILES, Tree, read_tree};

/// Whether `dir` holds only regular files and folders, within the limits. A link, a
/// socket or a `.git` folder anywhere inside means it isn't a plain copy: the digest
/// skips those, so the folder could hold something the digest doesn't cover.
fn plain(dir: &Path) -> bool {
    fn walk(dir: &Path, depth: usize, seen: &mut usize) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let Ok(entries) = fs::read_dir(dir) else {
            return false;
        };
        for entry in entries {
            *seen += 1;
            let Ok(entry) = entry else { return false };
            if *seen > MAX_FILES {
                return false;
            }
            let Ok(ft) = entry.file_type() else {
                return false;
            };
            if ft.is_dir() {
                if entry.file_name() == ".git" || !walk(&entry.path(), depth + 1, seen) {
                    return false;
                }
            } else if !ft.is_file() {
                return false;
            }
        }
        true
    }
    fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) && walk(dir, 0, &mut 0)
}

/// The folder's files and their digest, when it is a real folder that holds nothing the
/// digest would skip.
#[must_use]
pub fn exact(dir: &Path) -> Option<(Tree, String)> {
    if !plain(dir) {
        return None;
    }
    let tree = read_tree(dir).ok()?;
    let digest = tree.digest();
    Some((tree, digest))
}

/// The digest of a folder Atlas holds (a source in a local copy, reached through no
/// link), or None when it can't be read within the limits.
#[must_use]
pub fn digest(dir: &Path) -> Option<String> {
    read_tree(dir).ok().map(|t| t.digest())
}

/// Whether every file of `tree` is in `target` with the same bytes.
#[must_use]
pub fn held_by(tree: &Tree, target: &Path) -> bool {
    let Ok(theirs) = read_tree(target) else {
        return false;
    };
    let theirs: BTreeMap<&PathBuf, &Vec<u8>> =
        theirs.files.iter().map(|f| (&f.rel, &f.bytes)).collect();
    tree.files
        .iter()
        .all(|f| theirs.get(&f.rel).is_some_and(|b| **b == f.bytes))
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use super::*;

    #[test]
    fn only_plain_folders_have_a_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("SKILL.md"), "x").unwrap();
        fs::write(dir.join("scripts/run.sh"), "y").unwrap();
        let (tree, digest) = exact(&dir).unwrap();
        assert_eq!(digest, read_tree(&dir).unwrap().digest());
        assert!(held_by(&tree, &dir));

        symlink("/etc", dir.join("scripts/etc")).unwrap();
        assert!(exact(&dir).is_none(), "a link inside");
        fs::remove_file(dir.join("scripts/etc")).unwrap();
        fs::create_dir_all(dir.join(".git")).unwrap();
        assert!(exact(&dir).is_none(), "a clone with history");
        fs::remove_dir(dir.join(".git")).unwrap();
        symlink(&dir, tmp.path().join("via")).unwrap();
        assert!(
            exact(&tmp.path().join("via")).is_none(),
            "reached through a link"
        );

        let other = tmp.path().join("other");
        fs::create_dir_all(&other).unwrap();
        fs::write(other.join("SKILL.md"), "x").unwrap();
        assert!(!held_by(&tree, &other), "scripts/run.sh is missing there");
        fs::create_dir_all(other.join("scripts")).unwrap();
        fs::write(other.join("scripts/run.sh"), "y").unwrap();
        fs::write(other.join("more.md"), "z").unwrap();
        assert!(held_by(&tree, &other), "more files there are fine");
    }
}
