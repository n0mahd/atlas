//! The `skills` CLI's records of where an installed skill came from (`npx skills add
//! owner/repo`). Atlas reads them to group skills by source, and carries a skill's record
//! when the skill moves or is deleted, but only in a lock that sits in a repo Atlas commits.
//!
//! The format follows vercel-labs/skills v1.7.0 (`src/skill-lock.ts`, `src/local-lock.ts`),
//! as T3 Code's `SkillLockFiles` reads it:
//! - The global lock (version 3) is `$XDG_STATE_HOME/skills/.skill-lock.json` when that is
//!   set, else `~/.agents/.skill-lock.json`: `{"version": 3, "skills": {name: {source,
//!   sourceType, sourceUrl, ref?, skillPath?, skillFolderHash, installedAt, updatedAt}},
//!   "dismissed": {}}`, two-space JSON with no trailing newline. `skillFolderHash` is the
//!   GitHub tree SHA of the skill's folder; an empty one means "not version-tracked".
//! - A project lock (version 1) is `<repo>/skills-lock.json`: `{"version": 1, "skills":
//!   {name: {source, sourceType, ref?, skillPath?, computedHash}}}`, skills sorted by name,
//!   ending with a newline. `computedHash` is SHA-256 over the folder's files.
//! - The CLI treats a file it can't parse, or an older version, as empty and replaces it
//!   on its next write. So a lock that doesn't parse, or whose version isn't the one
//!   written here, is never rewritten (a newer one is still read).
//!
//! Nothing is written here: callers get the new text and write it inside a change.

use std::cmp::Ordering;
use std::fs;
use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha1::Digest as _;

pub const GLOBAL_LOCK_FILE: &str = ".skill-lock.json";
pub const PROJECT_LOCK_FILE: &str = "skills-lock.json";
const MAX_LOCK_BYTES: u64 = 4 * 1024 * 1024;
const MAX_HASHED_FILES: usize = 500;
const MAX_HASHED_BYTES: u64 = 16 * 1024 * 1024;

/// Which of the CLI's two locks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Global,
    Project,
}

impl LockKind {
    /// The version this module writes, and the oldest it reads.
    #[must_use]
    pub const fn version(self) -> u64 {
        match self {
            Self::Global => 3,
            Self::Project => 1,
        }
    }
}

/// Where the CLI keeps its global lock for a login whose home is `home`.
#[must_use]
pub fn global_lock_path(home: &Path, xdg_state_home: Option<&Path>) -> PathBuf {
    match xdg_state_home.filter(|p| p.is_absolute()) {
        Some(state) => state.join("skills").join(GLOBAL_LOCK_FILE),
        None => home.join(".agents").join(GLOBAL_LOCK_FILE),
    }
}

/// A lock that parsed.
#[derive(Clone, Debug)]
pub struct Lock {
    pub kind: LockKind,
    pub version: u64,
    data: Map<String, Value>,
    indent: String,
    trailing_newline: bool,
    sorted: bool,
}

#[derive(Clone, Debug)]
pub enum ReadLock {
    Missing,
    /// There is a file, but it isn't a lock Atlas can read (or a link, for a project lock).
    Unusable,
    Found(Lock),
}

impl ReadLock {
    #[must_use]
    pub const fn found(&self) -> Option<&Lock> {
        match self {
            Self::Found(l) => Some(l),
            _ => None,
        }
    }
}

/// Reads a lock. A project lock that is a link is unusable: Atlas never writes through a
/// link in a repo.
#[must_use]
pub fn read_lock(path: &Path, kind: LockKind) -> ReadLock {
    let meta = match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return ReadLock::Missing,
        Err(_) => return ReadLock::Unusable,
        Ok(m) => m,
    };
    let meta = if meta.file_type().is_symlink() {
        if kind == LockKind::Project {
            return ReadLock::Unusable;
        }
        match fs::metadata(path) {
            Ok(m) => m,
            Err(_) => return ReadLock::Unusable,
        }
    } else {
        meta
    };
    if !meta.is_file() || meta.len() > MAX_LOCK_BYTES {
        return ReadLock::Unusable;
    }
    let mut text = String::new();
    let read = fs::File::open(path).and_then(|f| f.take(MAX_LOCK_BYTES).read_to_string(&mut text));
    if read.is_err() {
        return ReadLock::Unusable;
    }
    parse_lock(&text, kind).map_or(ReadLock::Unusable, ReadLock::Found)
}

/// Parses a lock's text.
#[must_use]
pub fn parse_lock(text: &str, kind: LockKind) -> Option<Lock> {
    let Ok(Value::Object(data)) = serde_json::from_str::<Value>(text) else {
        return None;
    };
    let version = data.get("version")?.as_u64()?;
    let skills = data.get("skills")?.as_object()?;
    let names: Vec<&String> = skills.keys().collect();
    let sorted = names.windows(2).all(|w| w[0] <= w[1]);
    let indent = text
        .lines()
        .find_map(|l| {
            let trimmed = l.trim_start_matches([' ', '\t']);
            (trimmed.starts_with('"') && trimmed.len() < l.len())
                .then(|| l[..l.len() - trimmed.len()].to_owned())
        })
        .unwrap_or_else(|| "  ".to_owned());
    Some(Lock {
        kind,
        version,
        data,
        indent,
        trailing_newline: text.ends_with('\n'),
        sorted,
    })
}

/// `owner/repo` for a GitHub record, as the CLI accepts it.
#[must_use]
pub fn source_of(entry: &Value) -> Option<&str> {
    let source = entry.get("source")?.as_str()?;
    let github = entry.get("sourceType")?.as_str()? == "github";
    let plain = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    };
    let shaped = source
        .split_once('/')
        .is_some_and(|(o, r)| plain(o) && plain(r));
    (github && shaped).then_some(source)
}

impl Lock {
    /// An empty lock of `kind`, as the CLI would first write it.
    #[must_use]
    pub fn fresh(kind: LockKind) -> Self {
        let mut data = Map::new();
        data.insert("version".to_owned(), Value::from(kind.version()));
        data.insert("skills".to_owned(), Value::Object(Map::new()));
        if kind == LockKind::Global {
            data.insert("dismissed".to_owned(), Value::Object(Map::new()));
        }
        Self {
            kind,
            version: kind.version(),
            data,
            indent: "  ".to_owned(),
            trailing_newline: kind == LockKind::Project,
            sorted: true,
        }
    }

    /// Whether Atlas may rewrite it: it has exactly the version written here.
    #[must_use]
    pub const fn writable(&self) -> bool {
        self.version == self.kind.version()
    }

    fn skills(&self) -> Option<&Map<String, Value>> {
        self.data.get("skills").and_then(Value::as_object)
    }

    #[must_use]
    pub fn entry(&self, name: &str) -> Option<&Value> {
        self.skills()?.get(name)
    }

    /// GitHub sources by skill name. An older lock than the CLI writes says nothing.
    #[must_use]
    pub fn sources(&self) -> Vec<(String, String)> {
        if self.version < self.kind.version() {
            return Vec::new();
        }
        self.skills()
            .into_iter()
            .flatten()
            .filter_map(|(name, e)| source_of(e).map(|s| (name.clone(), s.to_owned())))
            .collect()
    }

    fn render(&self, skills: Map<String, Value>) -> String {
        let skills = if self.kind == LockKind::Project && self.sorted {
            let mut pairs: Vec<(String, Value)> = skills.into_iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            pairs.into_iter().collect()
        } else {
            skills
        };
        let mut data = self.data.clone();
        data.insert("skills".to_owned(), Value::Object(skills));
        let mut out = Vec::new();
        let fmt = serde_json::ser::PrettyFormatter::with_indent(self.indent.as_bytes());
        let mut ser = serde_json::Serializer::with_formatter(&mut out, fmt);
        let _ = serde::Serialize::serialize(&Value::Object(data), &mut ser);
        let mut text = String::from_utf8(out).unwrap_or_default();
        if self.trailing_newline {
            text.push('\n');
        }
        text
    }

    /// The file's text without `name`'s record.
    #[must_use]
    pub fn render_without(&self, name: &str) -> String {
        let mut skills = self.skills().cloned().unwrap_or_default();
        skills.shift_remove(name);
        self.render(skills)
    }

    /// The file's text with `name`'s record set to `entry`.
    #[must_use]
    pub fn render_with(&self, name: &str, entry: Value) -> String {
        let mut skills = self.skills().cloned().unwrap_or_default();
        skills.insert(name.to_owned(), entry);
        self.render(skills)
    }
}

/// The record as the other lock keeps it, or None when it can't be carried: a local
/// source means nothing elsewhere, a global record needs the URL the CLI reinstalls from,
/// and a project record needs the folder's `computedHash`, which is only known when the
/// folder is exactly the tree the global record names.
#[must_use]
pub fn convert(
    entry: &Value,
    from: LockKind,
    to: LockKind,
    hashes: Option<&FolderHashes>,
    now: &str,
) -> Option<Value> {
    let e = entry.as_object()?;
    let source = e.get("source")?.as_str()?;
    let source_type = e.get("sourceType")?.as_str()?;
    if matches!(source_type, "local" | "node_modules") {
        return None;
    }
    let text = |k: &str| e.get(k).and_then(Value::as_str);
    let mut out = Map::new();
    let mut put = |k: &str, v: Option<&str>| {
        if let Some(v) = v {
            out.insert(k.to_owned(), Value::from(v));
        }
    };
    let default_url = format!("https://github.com/{source}.git");
    match (from, to) {
        (LockKind::Project, LockKind::Project) | (LockKind::Global, LockKind::Global) => {
            return Some(entry.clone());
        }
        (LockKind::Project, LockKind::Global) => {
            let url = text("sourceUrl")
                .map(str::to_owned)
                .or_else(|| (source_type == "github").then(|| default_url.clone()))?;
            put("source", Some(source));
            put("sourceType", Some(source_type));
            put("sourceUrl", Some(&url));
            put("ref", text("ref"));
            put("skillPath", text("skillPath"));
            // The CLI's "not version-tracked" value, so it never overwrites the skill.
            put("skillFolderHash", Some(""));
            put("installedAt", Some(now));
            put("updatedAt", Some(now));
            put("wellKnownDigest", text("wellKnownDigest"));
        }
        (LockKind::Global, LockKind::Project) => {
            let recorded = text("skillFolderHash").filter(|h| !h.is_empty())?;
            let hashes = hashes?;
            if source_type != "github" || hashes.tree_sha != recorded {
                return None;
            }
            put("source", Some(source));
            put("sourceUrl", text("sourceUrl").filter(|u| *u != default_url));
            put("ref", text("ref"));
            put("sourceType", Some(source_type));
            put("skillPath", text("skillPath"));
            put("computedHash", Some(&hashes.computed_hash));
            put("wellKnownDigest", text("wellKnownDigest"));
        }
    }
    Some(Value::Object(out))
}

/// What the CLI's two kinds of hash say about a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderHashes {
    /// Git's tree SHA, which GitHub reports as the folder's `skillFolderHash`.
    pub tree_sha: String,
    /// The CLI's `computeSkillFolderHash`.
    pub computed_hash: String,
}

struct Entry {
    rel: String,
    link: bool,
    bytes: Vec<u8>,
    executable: bool,
}

fn read_folder(root: &Path) -> Option<Vec<Entry>> {
    let mut out = Vec::new();
    let mut total = 0_u64;
    let mut pending = vec![String::new()];
    while let Some(at) = pending.pop() {
        let dir = if at.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&at)
        };
        for entry in fs::read_dir(&dir).ok()? {
            let entry = entry.ok()?;
            let name = entry.file_name().into_string().ok()?;
            let rel = if at.is_empty() {
                name.clone()
            } else {
                format!("{at}/{name}")
            };
            let meta = fs::symlink_metadata(entry.path()).ok()?;
            if out.len() >= MAX_HASHED_FILES {
                return None;
            }
            if meta.file_type().is_symlink() {
                let target = fs::read_link(entry.path()).ok()?;
                out.push(Entry {
                    rel,
                    link: true,
                    bytes: target.to_string_lossy().into_owned().into_bytes(),
                    executable: false,
                });
            } else if meta.is_dir() {
                pending.push(rel);
            } else if meta.is_file() {
                total += meta.len();
                if total > MAX_HASHED_BYTES {
                    return None;
                }
                out.push(Entry {
                    rel,
                    link: false,
                    bytes: fs::read(entry.path()).ok()?,
                    executable: meta.permissions().mode() & 0o111 != 0,
                });
            } else {
                return None;
            }
        }
    }
    Some(out)
}

/// Both hashes of a folder, or None when it can't be read or is too big.
#[must_use]
pub fn hash_folder(folder: &Path) -> Option<FolderHashes> {
    let entries = read_folder(folder)?;
    Some(FolderHashes {
        tree_sha: tree_sha(&entries),
        computed_hash: computed_hash(&entries),
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            use std::fmt::Write as _;
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn object_sha(kind: &str, body: &[u8]) -> [u8; 20] {
    let mut h = sha1::Sha1::new();
    h.update(format!("{kind} {}\0", body.len()).as_bytes());
    h.update(body);
    h.finalize().into()
}

enum Node {
    Dir(std::collections::BTreeMap<String, Node>),
    Item { mode: &'static str, sha: [u8; 20] },
}

fn encode(node: &std::collections::BTreeMap<String, Node>) -> Vec<u8> {
    let mut items: Vec<(Vec<u8>, Vec<u8>, [u8; 20])> = node
        .iter()
        .map(|(name, child)| match child {
            Node::Dir(sub) => (
                format!("{name}/").into_bytes(),
                format!("40000 {name}\0").into_bytes(),
                object_sha("tree", &encode(sub)),
            ),
            Node::Item { mode, sha } => (
                name.clone().into_bytes(),
                format!("{mode} {name}\0").into_bytes(),
                *sha,
            ),
        })
        .collect();
    // Git sorts a folder as if its name ended with a slash.
    items.sort_by(|a, b| a.0.cmp(&b.0));
    items
        .into_iter()
        .flat_map(|(_, head, sha)| head.into_iter().chain(sha))
        .collect()
}

fn tree_sha(entries: &[Entry]) -> String {
    let mut root = std::collections::BTreeMap::new();
    for e in entries {
        let mut parts: Vec<&str> = e.rel.split('/').collect();
        let name = parts.pop().unwrap_or_default();
        let mut node = &mut root;
        for part in parts {
            let next = node
                .entry(part.to_owned())
                .or_insert_with(|| Node::Dir(std::collections::BTreeMap::new()));
            node = match next {
                Node::Dir(sub) => sub,
                Node::Item { .. } => return String::new(),
            };
        }
        let mode = if e.link {
            "120000"
        } else if e.executable {
            "100755"
        } else {
            "100644"
        };
        node.insert(
            name.to_owned(),
            Node::Item {
                mode,
                sha: object_sha("blob", &e.bytes),
            },
        );
    }
    hex(&object_sha("tree", &encode(&root)))
}

/// The CLI's `computeSkillFolderHash`: SHA-256 over each regular file's path and bytes,
/// sorted the way JavaScript's `localeCompare` sorts them, leaving out `.git` and
/// `node_modules`.
fn computed_hash(entries: &[Entry]) -> String {
    use sha2::Digest as _;
    let mut files: Vec<&Entry> = entries
        .iter()
        .filter(|e| {
            !e.link
                && !e
                    .rel
                    .split('/')
                    .rev()
                    .skip(1)
                    .any(|p| p == ".git" || p == "node_modules")
        })
        .collect();
    files.sort_by(|a, b| locale_cmp(&a.rel, &b.rel));
    let mut h = sha2::Sha256::new();
    for f in files {
        h.update(f.rel.as_bytes());
        h.update(&f.bytes);
    }
    hex(&h.finalize())
}

/// ICU's root order for the ASCII punctuation and symbols, which all come before digits,
/// which come before letters.
const PUNCTUATION: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";

fn primary(c: char) -> (u8, u32) {
    if c.is_whitespace() {
        (0, c as u32)
    } else if let Some(i) = PUNCTUATION.find(c) {
        (1, u32::try_from(i).unwrap_or(0))
    } else if c.is_ascii_digit() {
        (2, c as u32)
    } else if c.is_alphabetic() {
        (3, c.to_lowercase().next().unwrap_or(c) as u32)
    } else {
        (1, 64 + c as u32)
    }
}

/// An approximation of `String.prototype.localeCompare` in the root locale, enough for
/// file paths: punctuation, then digits, then letters ignoring case; ties broken by
/// lowercase first.
fn locale_cmp(a: &str, b: &str) -> Ordering {
    let pa = a.chars().map(primary);
    let pb = b.chars().map(primary);
    pa.cmp(pb).then_with(|| {
        let ta = a.chars().map(char::is_uppercase);
        let tb = b.chars().map(char::is_uppercase);
        ta.cmp(tb)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBAL: &str = r#"{
  "version": 3,
  "skills": {
    "pdf": {
      "source": "acme/tools",
      "sourceType": "github",
      "sourceUrl": "https://github.com/acme/tools.git",
      "skillPath": "skills/pdf/SKILL.md",
      "skillFolderHash": "",
      "installedAt": "2026-01-01T00:00:00.000Z",
      "updatedAt": "2026-01-01T00:00:00.000Z"
    },
    "mine": {
      "source": "./here",
      "sourceType": "local"
    }
  },
  "dismissed": {}
}"#;

    #[test]
    fn reads_sources_and_keeps_formatting_on_rewrite() {
        let lock = parse_lock(GLOBAL, LockKind::Global).unwrap();
        assert_eq!(
            lock.sources(),
            [("pdf".to_owned(), "acme/tools".to_owned())]
        );
        let text = lock.render_without("mine");
        assert!(!text.ends_with('\n'));
        assert!(
            text.starts_with("{\n  \"version\": 3,\n  \"skills\": {\n    \"pdf\""),
            "{text}"
        );
        assert!(text.ends_with("},\n  \"dismissed\": {}\n}"), "{text}");
        assert_eq!(
            lock.render_with("mine", lock.entry("mine").unwrap().clone()),
            GLOBAL
        );
    }

    #[test]
    fn old_or_broken_locks_are_not_written() {
        assert!(parse_lock("{", LockKind::Project).is_none());
        let old = parse_lock(r#"{"version": 2, "skills": {}}"#, LockKind::Global).unwrap();
        assert!(!old.writable());
        assert_eq!(old.sources(), []);
    }

    #[test]
    fn records_convert_between_locks() {
        let lock = parse_lock(GLOBAL, LockKind::Global).unwrap();
        let pdf = lock.entry("pdf").unwrap();
        // Untracked global records have no known hash, so they can't become project ones.
        assert!(convert(pdf, LockKind::Global, LockKind::Project, None, "now").is_none());
        let project = serde_json::json!({"source": "acme/tools", "sourceType": "github", "computedHash": "ab"});
        let global = convert(&project, LockKind::Project, LockKind::Global, None, "now").unwrap();
        assert_eq!(global["sourceUrl"], "https://github.com/acme/tools.git");
        assert_eq!(global["skillFolderHash"], "");
        let mut tracked = pdf.clone();
        tracked["skillFolderHash"] = Value::from("abc");
        let hashes = FolderHashes {
            tree_sha: "abc".to_owned(),
            computed_hash: "def".to_owned(),
        };
        let back = convert(
            &tracked,
            LockKind::Global,
            LockKind::Project,
            Some(&hashes),
            "now",
        )
        .unwrap();
        assert_eq!(back["computedHash"], "def");
        assert!(back.get("sourceUrl").is_none());
        let local = lock.entry("mine").unwrap();
        // A local path means nothing anywhere else.
        assert!(convert(local, LockKind::Global, LockKind::Global, None, "now").is_none());
        assert!(
            convert(
                local,
                LockKind::Global,
                LockKind::Project,
                Some(&hashes),
                "now"
            )
            .is_none()
        );
    }

    #[test]
    fn project_locks_stay_sorted_with_a_newline() {
        let lock = Lock::fresh(LockKind::Project);
        let text = lock.render_with(
            "zeta",
            serde_json::json!({"source": "a/b", "sourceType": "github"}),
        );
        let lock = parse_lock(&text, LockKind::Project).unwrap();
        let text = lock.render_with(
            "alpha",
            serde_json::json!({"source": "a/b", "sourceType": "github"}),
        );
        assert!(text.ends_with("}\n"));
        assert!(text.find("alpha").unwrap() < text.find("zeta").unwrap());
    }

    #[test]
    fn tree_sha_matches_git() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("x/scripts")).unwrap();
        fs::write(root.join("x/SKILL.md"), "---\nname: x\n---\n").unwrap();
        fs::write(root.join("x/README.md"), "readme\n").unwrap();
        fs::write(root.join("x/scripts/run.sh"), "echo hi\n").unwrap();
        fs::set_permissions(
            root.join("x/scripts/run.sh"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        std::os::unix::fs::symlink("SKILL.md", root.join("x/link.md")).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(root)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        git(&["init", "-q"]);
        git(&["add", "-A"]);
        let expected = git(&["write-tree", "--prefix=x/"]);
        assert_eq!(hash_folder(&root.join("x")).unwrap().tree_sha, expected);
    }

    #[test]
    fn locale_order_ignores_case_and_puts_punctuation_first() {
        let mut v = vec![
            "SKILL.md",
            "scripts/a.sh",
            "README.md",
            "references/b.md",
            "_x",
            "9",
        ];
        v.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(
            v,
            [
                "_x",
                "9",
                "README.md",
                "references/b.md",
                "scripts/a.sh",
                "SKILL.md"
            ]
        );
    }

    #[test]
    fn the_global_lock_follows_xdg_state_home() {
        let home = Path::new("/home/u");
        assert_eq!(
            global_lock_path(home, None),
            Path::new("/home/u/.agents/.skill-lock.json")
        );
        assert_eq!(
            global_lock_path(home, Some(Path::new("/s"))),
            Path::new("/s/skills/.skill-lock.json")
        );
    }
}
