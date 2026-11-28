//! The owner's project checkouts on this machine (decision 0027): found under
//! `project_roots` by their `origin` remote, and what git tracks in each. Everything is
//! read straight from `.git` (config and index), bounded, so a pass over dozens of
//! projects starts no git process and runs nothing from a repo's config.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use atlas_core::kernel::Slug;
use atlas_core::release::{ProjectRef, remote_key};
use atlas_harness::instructions::{EXCLUDE_BEGIN, EXCLUDE_END, exclude_with};

/// At most this many entries are looked at across all project roots.
pub const MAX_SCAN: usize = 500;
const MAX_CONFIG: u64 = 64 * 1024;
const MAX_INDEX: u64 = 64 * 1024 * 1024;
const MAX_EXCLUDE: u64 = 1024 * 1024;

/// One checkout of a registered project.
#[derive(Clone, Debug)]
pub struct Checkout {
    pub project: Slug,
    pub root: PathBuf,
    /// What git tracks there, or why that couldn't be read (Atlas then leaves the
    /// checkout alone).
    pub tracked: Result<Tracked, String>,
}

/// Paths git tracks in a checkout, relative to it, with `/` separators.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Tracked(BTreeSet<String>);

impl Tracked {
    /// Whether git tracks `rel` or anything inside it.
    #[must_use]
    pub fn covers(&self, rel: &str) -> bool {
        let rel = rel.trim_end_matches('/');
        self.0
            .range(rel.to_owned()..)
            .next()
            .is_some_and(|p| p == rel || p.strip_prefix(rel).is_some_and(|r| r.starts_with('/')))
    }
}

fn read_small(path: &Path, max: u64) -> Option<Vec<u8>> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.is_file() || meta.len() > max {
        return None;
    }
    let mut bytes = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(max)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// The values of a git config file, keyed `section.key` or `section.subsection.key`
/// (section and key in lower case). Enough for `remote.origin.url` and
/// `extensions.objectformat`; includes and conditional includes aren't followed.
fn parse_config(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut section = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            let Some(end) = line.find(']') else { continue };
            let inner = &line[1..end];
            section = match inner.split_once(char::is_whitespace) {
                Some((name, sub)) => {
                    let sub = sub.trim().trim_matches('"');
                    format!("{}.{sub}", name.to_ascii_lowercase())
                }
                None => inner.to_ascii_lowercase(),
            };
            continue;
        }
        if line.is_empty() || line.starts_with(['#', ';']) || section.is_empty() {
            continue;
        }
        let (key, value) = line.split_once('=').unwrap_or((line, "true"));
        // A comment starts at `#` or `;` outside quotes; quotes themselves go.
        let mut clean = String::new();
        let mut quoted = false;
        for c in value.trim().chars() {
            match c {
                '"' => quoted = !quoted,
                '#' | ';' if !quoted => break,
                c => clean.push(c),
            }
        }
        let value = clean.trim();
        out.insert(
            format!("{section}.{}", key.trim().to_ascii_lowercase()),
            value.to_owned(),
        );
    }
    out
}

/// A repo's `.git/config`, when `dir` is a checkout with its own `.git` folder (worktrees
/// and submodules, whose `.git` is a file, aren't followed).
fn git_config(dir: &Path) -> Option<BTreeMap<String, String>> {
    let git = dir.join(".git");
    if !fs::symlink_metadata(&git).is_ok_and(|m| m.is_dir()) {
        return None;
    }
    let bytes = read_small(&git.join("config"), MAX_CONFIG)?;
    Some(parse_config(&String::from_utf8_lossy(&bytes)))
}

/// The `origin` remote of the checkout at `dir`, read from its `.git/config`.
#[must_use]
pub fn origin(dir: &Path) -> Option<String> {
    git_config(dir)?.remove("remote.origin.url")
}

/// The checkouts of `projects` that are direct children of `roots`, matched by their
/// `origin` remote ([`remote_key`]). Links are not followed; at most [`MAX_SCAN`]
/// entries are looked at. A project can have several checkouts.
#[must_use]
pub fn find(roots: &[PathBuf], projects: &[ProjectRef]) -> (Vec<Checkout>, Vec<String>) {
    let mut notes = Vec::new();
    let wanted: BTreeMap<String, &Slug> = projects
        .iter()
        .map(|p| (remote_key(&p.remote), &p.name))
        .collect();
    let mut found = Vec::new();
    if wanted.is_empty() {
        return (found, notes);
    }
    let mut seen = 0;
    'roots: for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let mut children: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            seen += 1;
            if seen > MAX_SCAN {
                notes.push(format!(
                    "looked for project checkouts in only the first {MAX_SCAN} folders"
                ));
                break 'roots;
            }
            let path = entry.path();
            if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                children.push(path);
            }
        }
        children.sort();
        for dir in children {
            let Some(config) = git_config(&dir) else {
                continue;
            };
            let Some(project) = config
                .get("remote.origin.url")
                .and_then(|url| wanted.get(&remote_key(url)))
            else {
                continue;
            };
            let sha256 = config
                .get("extensions.objectformat")
                .is_some_and(|f| f.eq_ignore_ascii_case("sha256"));
            let tracked = tracked(&dir.join(".git/index"), if sha256 { 32 } else { 20 });
            found.push(Checkout {
                project: (*project).clone(),
                root: dir,
                tracked,
            });
        }
    }
    (found, notes)
}

/// Reads git's index (versions 2 to 4) for the paths it tracks. A missing index means
/// nothing is tracked yet.
///
/// # Errors
/// Unreadable, too big, an unknown version, a split index, or not an index at all.
pub fn tracked(index: &Path, hash_len: usize) -> Result<Tracked, String> {
    if fs::symlink_metadata(index).is_err() {
        return Ok(Tracked::default());
    }
    let bytes = read_small(index, MAX_INDEX).ok_or("its git index can't be read")?;
    parse_index(&bytes, hash_len).map(Tracked)
}

fn parse_index(b: &[u8], hash_len: usize) -> Result<BTreeSet<String>, String> {
    let bad = || "its git index isn't one Atlas can read".to_owned();
    let u32_at = |at: usize| -> Result<usize, String> {
        let s = b.get(at..at + 4).ok_or_else(bad)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]) as usize)
    };
    if b.get(..4) != Some(b"DIRC") {
        return Err(bad());
    }
    let version = u32_at(4)?;
    if !(2..=4).contains(&version) {
        return Err(bad());
    }
    let count = u32_at(8)?;
    let mut at = 12;
    let mut out = BTreeSet::new();
    let mut prev: Vec<u8> = Vec::new();
    for _ in 0..count {
        let start = at;
        at += 40 + hash_len;
        let flags = b.get(at..at + 2).ok_or_else(bad)?;
        let flags = u16::from_be_bytes([flags[0], flags[1]]);
        at += 2;
        if version >= 3 && flags & 0x4000 != 0 {
            at += 2;
        }
        let fixed = at - start;
        let name = if version == 4 {
            // The part of the previous path to drop, as an offset varint, then the rest.
            let mut c = *b.get(at).ok_or_else(bad)?;
            at += 1;
            let mut drop = usize::from(c & 0x7f);
            while c & 0x80 != 0 {
                c = *b.get(at).ok_or_else(bad)?;
                at += 1;
                drop = drop
                    .checked_add(1)
                    .and_then(|d| d.checked_mul(128))
                    .ok_or_else(bad)?
                    | usize::from(c & 0x7f);
            }
            let keep = prev.len().checked_sub(drop).ok_or_else(bad)?;
            let end = b[at..].iter().position(|&x| x == 0).ok_or_else(bad)? + at;
            let mut name = prev[..keep].to_vec();
            name.extend_from_slice(&b[at..end]);
            at = end + 1;
            name
        } else {
            let end = b
                .get(at..)
                .and_then(|r| r.iter().position(|&x| x == 0))
                .ok_or_else(bad)?
                + at;
            let name = b[at..end].to_vec();
            at = start + ((fixed + name.len() + 8) & !7);
            name
        };
        out.insert(String::from_utf8_lossy(&name).into_owned());
        prev = name;
    }
    // Extensions follow; a split index keeps most entries in another file.
    let end = b.len().saturating_sub(hash_len);
    while at + 8 <= end {
        if &b[at..at + 4] == b"link" {
            return Err("it uses a split git index, which Atlas can't read".to_owned());
        }
        at += 8 + u32_at(at + 4)?;
    }
    Ok(out)
}

/// The lines of Atlas's block in `.git/info/exclude` (between
/// [`EXCLUDE_BEGIN`] and [`EXCLUDE_END`]) that name paths, as paths in the checkout.
/// The block is shared: the hub puts `/CLAUDE.local.md` there too.
#[must_use]
pub fn listed(exclude: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut inside = false;
    for line in exclude.lines() {
        match line.trim_end() {
            EXCLUDE_BEGIN => inside = true,
            EXCLUDE_END => inside = false,
            l if inside => {
                if let Some(p) = l.strip_prefix('/') {
                    out.insert(p.to_owned());
                }
            }
            _ => {}
        }
    }
    out
}

/// `exclude` with `paths` added to Atlas's block ([`exclude_with`]: add-only, every other
/// line kept), or None when the block already lists them all.
#[must_use]
pub fn with_paths(exclude: &str, paths: &BTreeSet<String>) -> Option<String> {
    let have = listed(exclude);
    let add: Vec<String> = paths
        .iter()
        .filter(|p| !have.contains(*p))
        .map(|p| format!("/{p}"))
        .collect();
    if add.is_empty() {
        return None;
    }
    let add: Vec<&str> = add.iter().map(String::as_str).collect();
    Some(exclude_with(exclude, &add))
}

/// The current `.git/info/exclude`, or why Atlas won't edit it.
///
/// # Errors
/// It is a link, not a regular file, or too big.
pub fn read_exclude(root: &Path) -> Result<String, String> {
    let path = root.join(".git/info/exclude");
    match fs::symlink_metadata(&path) {
        Err(_) => Ok(String::new()),
        Ok(m) if m.is_file() => read_small(&path, MAX_EXCLUDE)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .ok_or_else(|| "its .git/info/exclude is too big".to_owned()),
        Ok(_) => Err("its .git/info/exclude isn't a regular file".to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {out:?}");
        String::from_utf8(out.stdout).unwrap()
    }

    fn repo_with_files(dir: &Path) {
        fs::create_dir_all(dir).unwrap();
        git(dir, &["init", "-q"]);
        for f in [
            "AGENTS.md",
            ".claude/skills/kept/SKILL.md",
            ".agents/skills/a/SKILL.md",
            ".agents/skills/a/scripts/run-something-with-a-long-name.sh",
            "src/main.rs",
            "zed.txt",
        ] {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "x").unwrap();
        }
        git(dir, &["add", "-A"]);
    }

    fn ls_files(dir: &Path) -> BTreeSet<String> {
        git(dir, &["ls-files"]).lines().map(str::to_owned).collect()
    }

    #[test]
    fn the_index_lists_what_git_tracks_in_every_version() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("r");
        repo_with_files(&dir);
        // An intent-to-add entry needs the extended flags of version 3.
        fs::write(dir.join("later.md"), "x").unwrap();
        git(&dir, &["add", "-N", "later.md"]);
        for version in ["2", "3", "4"] {
            git(&dir, &["update-index", "--index-version", version]);
            let got = tracked(&dir.join(".git/index"), 20).unwrap();
            assert_eq!(got.0, ls_files(&dir), "version {version}");
        }
        let t = tracked(&dir.join(".git/index"), 20).unwrap();
        assert!(t.covers(".claude/skills/kept"));
        assert!(t.covers(".claude/skills"));
        assert!(t.covers("zed.txt"));
        assert!(!t.covers(".claude/skills/new"));
        assert!(
            !t.covers(".claude/skill"),
            "a prefix of a name isn't inside it"
        );
        assert!(!t.covers("zed"));
        assert_eq!(
            tracked(&tmp.path().join("none/.git/index"), 20),
            Ok(Tracked::default())
        );
        fs::write(tmp.path().join("junk"), "DIRCnope").unwrap();
        assert!(tracked(&tmp.path().join("junk"), 20).is_err());
    }

    #[test]
    fn checkouts_are_found_by_their_origin_in_any_form() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("Projects");
        for (name, url) in [
            ("shop", "git@git.example:me/shop.git"),
            ("shop-2", "https://git.example/me/shop/"),
            ("other", "git@git.example:me/other.git"),
        ] {
            let dir = root.join(name);
            fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q"]);
            git(&dir, &["remote", "add", "origin", url]);
        }
        fs::create_dir_all(root.join("plain")).unwrap();
        std::os::unix::fs::symlink(root.join("shop"), root.join("via-link")).unwrap();
        let projects = [ProjectRef {
            name: Slug::new("shop").unwrap(),
            remote: "ssh://git@git.example/me/shop".into(),
        }];
        let (found, notes) = find(&[root.clone(), tmp.path().join("missing")], &projects);
        let roots: Vec<_> = found.iter().map(|c| c.root.clone()).collect();
        assert_eq!(roots, [root.join("shop"), root.join("shop-2")]);
        assert!(found.iter().all(|c| c.tracked == Ok(Tracked::default())));
        assert_eq!(notes, Vec::<String>::new());
        assert_eq!(
            origin(&root.join("other")).as_deref(),
            Some("git@git.example:me/other.git")
        );
    }

    #[test]
    fn config_values_are_read_like_git_reads_them() {
        let c = parse_config(
            "[core]\n\tbare = false\n[remote \"origin\"]\n\turl = \"git@h:o/r.git\" # mine\n\
             \tfetch = +refs/heads/*:refs/remotes/origin/*\n[Extensions]\n\tobjectFormat = sha256\n",
        );
        assert_eq!(c["remote.origin.url"], "git@h:o/r.git");
        assert_eq!(c["extensions.objectformat"], "sha256");
        assert_eq!(c["core.bare"], "false");
    }

    #[test]
    fn links_are_added_to_the_shared_block_and_nothing_is_rewritten() {
        let mine = "# my own\n*.log\n";
        let paths: BTreeSet<String> =
            [".claude/skills/a".to_owned(), ".claude/skills/b".to_owned()]
                .into_iter()
                .collect();
        let with = with_paths(mine, &paths).unwrap();
        assert!(with.starts_with(mine), "{with}");
        assert_eq!(listed(&with), paths);
        assert_eq!(with_paths(&with, &paths), None, "idempotent");

        // The hub's own entry stays, and a link that is gone isn't pruned.
        let shared = format!("{EXCLUDE_BEGIN}\n/CLAUDE.local.md\n{EXCLUDE_END}\nbuild/\n");
        let one: BTreeSet<String> = [".claude/skills/a".to_owned()].into_iter().collect();
        let added = with_paths(&shared, &one).unwrap();
        assert_eq!(
            added,
            format!(
                "{EXCLUDE_BEGIN}\n/CLAUDE.local.md\n/.claude/skills/a\n{EXCLUDE_END}\nbuild/\n"
            )
        );
        assert_eq!(with_paths(&added, &BTreeSet::new()), None);
        assert!(listed(&added).contains("CLAUDE.local.md"));
    }
}
