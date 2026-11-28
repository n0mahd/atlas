//! The pure part of updating a skill: finding its folder in a source listing, what an
//! update does to each file, the three-way merge and the diff shown before anything is
//! written. A folder is compared as a map from each file's path to its git blob SHA and
//! execute bit, so a local folder and a GitHub listing compare without downloading
//! anything, and a file that only became executable counts as changed.

use std::collections::BTreeMap;

use atlas_fs::lock::{GitHubSource, is_ignored_path, is_safe_rel};

use super::github::{Entry, RepoTree};

/// The compared files of a folder: path → blob SHA and mode. Ignored paths are left out.
pub(crate) type Side = BTreeMap<String, UpFile>;

/// A file of a folder, relative to that folder: its blob SHA and whether it is executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UpFile {
    pub sha: String,
    pub executable: bool,
}

/// A skill's folder in its source at one version.
#[derive(Clone, Debug)]
pub(crate) struct Upstream {
    /// The folder's git tree SHA, what a global lock record stores.
    pub sha: String,
    /// Every regular file under it, by relative path, ignored ones included.
    pub files: BTreeMap<String, UpFile>,
    pub side: Side,
    /// It holds links, submodules or names Atlas won't write.
    pub unsupported: bool,
}

/// The folder a record's skill is in, with its tree SHA. With no path in the record (older
/// records), a folder named like the skill holding a `SKILL.md`, shallowest first.
pub(crate) fn find_folder(
    tree: &RepoTree,
    src: &GitHubSource,
    name: &str,
) -> Option<(String, String)> {
    let sha_of = |folder: &str| {
        if folder.is_empty() {
            Some(tree.sha.clone())
        } else {
            tree.tree
                .iter()
                .find(|e| e.kind == "tree" && e.path == folder)
                .map(|e| e.sha.clone())
        }
    };
    if let Some(folder) = &src.folder {
        return sha_of(folder).map(|sha| (folder.clone(), sha));
    }
    let mut candidates: Vec<&str> = tree
        .tree
        .iter()
        .filter(|e| e.kind == "blob")
        .filter_map(|e| {
            let (dir, file) = e.path.rsplit_once('/')?;
            file.eq_ignore_ascii_case("skill.md").then_some(dir)
        })
        .filter(|dir| dir.rsplit('/').next() == Some(name))
        .collect();
    candidates.sort_by_key(|d| (d.split('/').count(), d.to_string()));
    let folder = candidates.first()?.to_string();
    sha_of(&folder).map(|sha| (folder, sha))
}

/// The entries under `folder` (relative to it) as an upstream version with tree SHA `sha`.
pub(crate) fn upstream_of(entries: &[Entry], folder: &str, sha: &str) -> Upstream {
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut files = BTreeMap::new();
    let mut unsupported = false;
    for e in entries.iter().filter(|e| e.kind != "tree") {
        let Some(rel) = e.path.strip_prefix(&prefix) else {
            continue;
        };
        if is_ignored_path(rel) {
            continue;
        }
        let regular = e.kind == "blob" && (e.mode == "100644" || e.mode == "100755");
        if !regular || !is_safe_rel(rel) || !super::github::is_sha(&e.sha) {
            unsupported = true;
            continue;
        }
        files.insert(
            rel.to_owned(),
            UpFile {
                sha: e.sha.clone(),
                executable: e.mode == "100755",
            },
        );
    }
    // Ignored files are kept for the CLI's hash, which counts some of them.
    for e in entries
        .iter()
        .filter(|e| e.kind == "blob" && e.mode != "120000")
    {
        if let Some(rel) = e.path.strip_prefix(&prefix)
            && is_ignored_path(rel)
            && is_safe_rel(rel)
            && super::github::is_sha(&e.sha)
        {
            files.insert(
                rel.to_owned(),
                UpFile {
                    sha: e.sha.clone(),
                    executable: e.mode == "100755",
                },
            );
        }
    }
    let side = files
        .iter()
        .filter(|(p, _)| !is_ignored_path(p))
        .map(|(p, f)| (p.clone(), f.clone()))
        .collect();
    Upstream {
        sha: sha.to_owned(),
        files,
        side,
        unsupported,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    Added,
    Modified,
    Removed,
}

/// What an update does with one file that differs between yours and the source's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Only the source changed it: theirs is taken.
    Theirs,
    /// Only you changed it: yours stays.
    Mine,
    /// Both changed it: merged line by line, which may still clash.
    Merge,
    /// Both changed it in ways a merge can't settle, or nothing says which side did.
    Clash,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Planned {
    pub path: String,
    /// From your copy to the source's.
    pub change: Change,
    pub outcome: Outcome,
}

/// Each file that differs between `mine` and `theirs`, and what an update does with it.
/// Without `base` (the installed version) no file can be said to be only one side's
/// change, so every difference clashes. `SKILL.md` first, then by path.
pub(crate) fn plan_files(mine: &Side, theirs: &Side, base: Option<&Side>) -> Vec<Planned> {
    let mut paths: Vec<&String> = mine.keys().chain(theirs.keys()).collect();
    paths.sort();
    paths.dedup();
    let mut out: Vec<Planned> = paths
        .into_iter()
        .filter_map(|path| {
            let m = mine.get(path);
            let t = theirs.get(path);
            if m == t {
                return None;
            }
            let change = match (m, t) {
                (None, _) => Change::Added,
                (_, None) => Change::Removed,
                _ => Change::Modified,
            };
            let outcome = match base {
                None => Outcome::Clash,
                Some(base) => {
                    let b = base.get(path);
                    if m == b {
                        Outcome::Theirs
                    } else if t == b {
                        Outcome::Mine
                    } else if m.is_some() && t.is_some() && b.is_some() {
                        Outcome::Merge
                    } else {
                        Outcome::Clash
                    }
                }
            };
            Some(Planned {
                path: path.clone(),
                change,
                outcome,
            })
        })
        .collect();
    out.sort_by_key(|p| (!p.path.eq_ignore_ascii_case("skill.md"), p.path.clone()));
    out
}

/// A file larger than this is never merged; both sides changing it clashes.
const MAX_MERGE_BYTES: usize = 4 * 1024 * 1024;

/// Bytes that look like a binary file: a NUL early on, or not UTF-8.
pub(crate) fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0) || std::str::from_utf8(bytes).is_err()
}

/// Your edits and the source's merged line by line from the version both started from;
/// `None` when they clash (two changes to the same lines) or a side isn't small text.
pub(crate) fn merge3(base: &[u8], mine: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    let sides = [base, mine, theirs];
    if sides
        .iter()
        .any(|s| s.len() > MAX_MERGE_BYTES || is_binary(s))
    {
        return None;
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
    diffy::merge(&text(base), &text(mine), &text(theirs))
        .ok()
        .map(String::into_bytes)
}

/// One line of a unified diff, with its kind for styling.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DiffLine {
    /// `add`, `del`, `hunk` or `ctx`.
    pub class: &'static str,
    pub text: String,
}

/// The largest text a diff is shown for.
pub(crate) const MAX_SHOWN_BYTES: usize = 200 * 1024;

/// A unified diff from `old` to `new` (either absent for an added or removed file), or
/// `None` when a side is binary or too big to show.
pub(crate) fn unified(old: Option<&[u8]>, new: Option<&[u8]>) -> Option<Vec<DiffLine>> {
    let old = old.unwrap_or_default();
    let new = new.unwrap_or_default();
    if old.len() > MAX_SHOWN_BYTES
        || new.len() > MAX_SHOWN_BYTES
        || is_binary(old)
        || is_binary(new)
    {
        return None;
    }
    let (o, n) = (String::from_utf8_lossy(old), String::from_utf8_lossy(new));
    let patch = diffy::create_patch(&o, &n).to_string();
    Some(
        patch
            .lines()
            .skip_while(|l| l.starts_with("---") || l.starts_with("+++"))
            .map(|l| DiffLine {
                class: match l.as_bytes().first() {
                    Some(b'@') => "hunk",
                    Some(b'+') => "add",
                    Some(b'-') => "del",
                    _ => "ctx",
                },
                text: l.to_owned(),
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn side(files: &[(&str, &str)]) -> Side {
        files
            .iter()
            .map(|(p, s)| {
                let executable = s.ends_with('x');
                let sha = s.trim_end_matches('x').to_owned();
                ((*p).to_owned(), UpFile { sha, executable })
            })
            .collect()
    }

    #[test]
    fn a_mode_change_is_a_change() {
        // run.sh keeps its bytes upstream but becomes executable (`x`).
        let base = side(&[("SKILL.md", "b1"), ("run.sh", "r1")]);
        let theirs = side(&[("SKILL.md", "b1"), ("run.sh", "r1x")]);
        let plan = plan_files(&base, &theirs, Some(&base));
        assert_eq!(
            plan,
            [Planned {
                path: "run.sh".to_owned(),
                change: Change::Modified,
                outcome: Outcome::Theirs,
            }]
        );
        // The owner made it executable themselves: theirs is unchanged, so it's theirs.
        let mine = side(&[("SKILL.md", "b1"), ("run.sh", "r1x")]);
        assert_eq!(
            plan_files(&mine, &base, Some(&base))[0].outcome,
            Outcome::Mine
        );
    }

    #[test]
    fn plans_follow_who_changed_what() {
        let base = side(&[
            ("SKILL.md", "b1"),
            ("a.md", "a1"),
            ("old.md", "o1"),
            ("both.md", "x1"),
        ]);
        let mine = side(&[
            ("SKILL.md", "b1"),
            ("a.md", "a2"),
            ("old.md", "o1"),
            ("both.md", "x2"),
        ]);
        let theirs = side(&[
            ("SKILL.md", "b2"),
            ("a.md", "a1"),
            ("new.md", "n1"),
            ("both.md", "x3"),
        ]);
        let plan = plan_files(&mine, &theirs, Some(&base));
        let got: Vec<(&str, Change, Outcome)> = plan
            .iter()
            .map(|p| (p.path.as_str(), p.change, p.outcome))
            .collect();
        assert_eq!(
            got,
            [
                ("SKILL.md", Change::Modified, Outcome::Theirs),
                ("a.md", Change::Modified, Outcome::Mine),
                ("both.md", Change::Modified, Outcome::Merge),
                ("new.md", Change::Added, Outcome::Theirs),
                ("old.md", Change::Removed, Outcome::Theirs),
            ]
        );
        // Without a base every difference is the owner's call.
        assert!(
            plan_files(&mine, &theirs, None)
                .iter()
                .all(|p| p.outcome == Outcome::Clash)
        );
    }

    #[test]
    fn diff3_table() {
        let base = "one\ntwo\nthree\nfour\nfive\n";
        let cases: [(&str, &str, &str, Option<&str>); 4] = [
            // Unedited: yours is the base, so the merge is theirs.
            (
                "unedited",
                base,
                "one\ntwo\nTHREE\nfour\nfive\n",
                Some("one\ntwo\nTHREE\nfour\nfive\n"),
            ),
            // Edited, no overlap: both changes kept.
            (
                "apart",
                "ONE\ntwo\nthree\nfour\nfive\n",
                "one\ntwo\nthree\nfour\nFIVE\n",
                Some("ONE\ntwo\nthree\nfour\nFIVE\n"),
            ),
            // Both changed the same line differently: a clash.
            (
                "clash",
                "one\ntwo\nmine\nfour\nfive\n",
                "one\ntwo\ntheirs\nfour\nfive\n",
                None,
            ),
            // The same change on both sides isn't a clash.
            (
                "same",
                "one\ntwo\nX\nfour\nfive\n",
                "one\ntwo\nX\nfour\nfive\n",
                Some("one\ntwo\nX\nfour\nfive\n"),
            ),
        ];
        for (name, mine, theirs, want) in cases {
            let got = merge3(base.as_bytes(), mine.as_bytes(), theirs.as_bytes());
            assert_eq!(got.as_deref(), want.map(str::as_bytes), "{name}");
        }
        assert_eq!(
            merge3(b"a\0b", b"a\0c", b"a\0d"),
            None,
            "binary never merges"
        );
    }

    #[test]
    fn diffs_are_line_classed() {
        let lines = unified(Some(b"a\nb\n"), Some(b"a\nc\n")).unwrap();
        let classes: Vec<&str> = lines.iter().map(|l| l.class).collect();
        assert_eq!(classes, ["hunk", "ctx", "del", "add"]);
        assert_eq!(lines[3].text, "+c");
        assert!(unified(None, Some(b"\0bin")).is_none());
        assert_eq!(
            unified(None, Some(b"x\n")).unwrap().last().unwrap().text,
            "+x"
        );
    }

    fn entry(path: &str, kind: &str, mode: &str) -> Entry {
        Entry {
            path: path.to_owned(),
            mode: mode.to_owned(),
            kind: kind.to_owned(),
            sha: "a".repeat(40),
        }
    }

    #[test]
    fn listed_paths_are_sanitised() {
        let ok = [
            entry("skills/x", "tree", "040000"),
            entry("skills/x/SKILL.md", "blob", "100644"),
            entry("skills/x/run.sh", "blob", "100755"),
            entry("skills/x/node_modules/a.js", "blob", "100644"),
            entry("skills/y/SKILL.md", "blob", "100644"),
        ];
        let up = upstream_of(&ok, "skills/x", &"b".repeat(40));
        assert!(!up.unsupported);
        assert_eq!(up.side.keys().collect::<Vec<_>>(), ["SKILL.md", "run.sh"]);
        assert!(up.files["run.sh"].executable);
        assert!(up.files.contains_key("node_modules/a.js"));

        for bad in [
            entry("skills/x/../../etc/passwd", "blob", "100644"),
            entry("skills/x/a\\b", "blob", "100644"),
            entry("skills/x/link", "blob", "120000"),
            entry("skills/x/sub", "commit", "160000"),
            entry("skills/x//a", "blob", "100644"),
            entry("skills/x/a\nb", "blob", "100644"),
        ] {
            let mut list = ok.to_vec();
            list.push(bad.clone());
            let up = upstream_of(&list, "skills/x", &"b".repeat(40));
            assert!(up.unsupported, "{:?}", bad.path);
            assert!(up.files.keys().all(|p| is_safe_rel(p)));
        }
    }

    #[test]
    fn folders_are_found_by_path_or_name() {
        let tree = RepoTree {
            sha: "r".repeat(40),
            truncated: false,
            tree: vec![
                Entry {
                    sha: "1".repeat(40),
                    ..entry("deep/a/x", "tree", "040000")
                },
                entry("deep/a/x/SKILL.md", "blob", "100644"),
                Entry {
                    sha: "2".repeat(40),
                    ..entry("x", "tree", "040000")
                },
                entry("x/SKILL.md", "blob", "100644"),
            ],
        };
        let src = |folder: Option<&str>| GitHubSource {
            owner: "o".to_owned(),
            repo: "r".to_owned(),
            folder: folder.map(str::to_owned),
            git_ref: None,
        };
        assert_eq!(
            find_folder(&tree, &src(None), "x"),
            Some(("x".to_owned(), "2".repeat(40)))
        );
        assert_eq!(
            find_folder(&tree, &src(Some("deep/a/x")), "x"),
            Some(("deep/a/x".to_owned(), "1".repeat(40)))
        );
        assert_eq!(
            find_folder(&tree, &src(Some("")), "x").unwrap().1,
            "r".repeat(40)
        );
        assert_eq!(find_folder(&tree, &src(Some("gone")), "x"), None);
    }
}
