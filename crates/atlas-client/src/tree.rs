//! Fingerprints of real folders, to tell an exact copy of what Atlas holds (which atlasd
//! may replace with its link) from anything else (which it never touches). Bounded, and
//! never follows a link.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_fs::library::{Tree, read_tree};

/// The folder's files and their exact digest ([`Tree::exact_digest`]), when it is a real
/// folder (not reached through a link) holding nothing but plain files and folders.
#[must_use]
pub fn exact(dir: &Path) -> Option<(Tree, String)> {
    if !fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let tree = read_tree(dir).ok()?;
    let digest = tree.exact_digest()?;
    Some((tree, digest))
}

/// The exact digest of a folder Atlas holds, such as a skill in a local copy, or None
/// when it can't be read within the limits or holds a link.
#[must_use]
pub fn digest(dir: &Path) -> Option<String> {
    read_tree(dir).ok()?.exact_digest()
}

/// How much of a folder's content another folder holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Held {
    /// Some file is missing there or has other bytes.
    Not,
    /// Every file is there with the same bytes, but some execute bit differs.
    Bytes,
    /// Every file is there with the same bytes and execute bit: the folder can be made
    /// again from it byte for byte (more files there are fine).
    Exactly,
}

/// How `target` holds the files of `tree`.
#[must_use]
pub fn held_by(tree: &Tree, target: &Path) -> Held {
    let Ok(theirs) = read_tree(target) else {
        return Held::Not;
    };
    let theirs: BTreeMap<&PathBuf, (&Vec<u8>, bool)> = theirs
        .files
        .iter()
        .map(|f| (&f.rel, (&f.bytes, f.executable)))
        .collect();
    let mut held = Held::Exactly;
    for f in &tree.files {
        match theirs.get(&f.rel) {
            Some((b, x)) if **b == f.bytes => {
                if *x != f.executable {
                    held = Held::Bytes;
                }
            }
            _ => return Held::Not,
        }
    }
    held
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use super::*;

    #[test]
    fn only_plain_folders_have_a_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skill");
        fs::create_dir_all(dir.join("scripts")).unwrap();
        fs::write(dir.join("SKILL.md"), "x").unwrap();
        fs::write(dir.join("scripts/run.sh"), "y").unwrap();
        let (tree, digest) = exact(&dir).unwrap();
        assert_eq!(Some(digest), read_tree(&dir).unwrap().exact_digest());
        assert_eq!(held_by(&tree, &dir), Held::Exactly);

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
        assert_eq!(
            held_by(&tree, &other),
            Held::Not,
            "scripts/run.sh is missing there"
        );
        fs::create_dir_all(other.join("scripts")).unwrap();
        fs::write(other.join("scripts/run.sh"), "y").unwrap();
        fs::write(other.join("more.md"), "z").unwrap();
        assert_eq!(
            held_by(&tree, &other),
            Held::Exactly,
            "more files there are fine"
        );
        fs::set_permissions(
            other.join("scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        assert_eq!(
            held_by(&tree, &other),
            Held::Bytes,
            "an execute bit differs"
        );
    }
}
