//! The `skills` CLI's lock records (`npx skills`): where each installed skill came from
//! and which version of it was installed. Read only, bounded; the one write Atlas makes
//! goes through [`LockFile::with_baseline`], which returns new text for the caller to
//! commit.
//!
//! The format follows the CLI (vercel-labs/skills v1.7.0, `src/skill-lock.ts` and
//! `src/local-lock.ts`), as the owner's T3 Code reads it:
//! - **Global lock** (version 3): `$XDG_STATE_HOME/skills/.skill-lock.json` when that is
//!   set, else `~/.agents/.skill-lock.json`. Written as `JSON.stringify(lock, null, 2)`
//!   with no trailing newline. A record's `skillFolderHash` is the GitHub tree SHA of the
//!   skill's folder; an empty one means "not version-tracked".
//! - **Project lock** (version 1): `<repo>/skills-lock.json`, skills sorted by name, with a
//!   trailing newline. A record's `computedHash` is SHA-256 over the folder's files
//!   ([`computed_hash`]).
//!
//! The CLI treats a lock it can't parse, or one older than it knows, as empty and its next
//! write replaces it, so Atlas never rewrites such a file; a newer version is read but not
//! written. A rewrite keeps the file's indent, trailing newline, key order and record order.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use sha1::Digest as _;

/// The global lock's file name and the version Atlas reads and writes.
pub const GLOBAL_LOCK_FILE: &str = ".skill-lock.json";
pub const GLOBAL_LOCK_VERSION: u64 = 3;
/// The project lock's file name (at the repo's top) and its version.
pub const PROJECT_LOCK_FILE: &str = "skills-lock.json";
pub const PROJECT_LOCK_VERSION: u64 = 1;
/// The largest lock file read.
pub const MAX_LOCK_BYTES: u64 = 4 * 1024 * 1024;

/// Which of the two locks a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LockKind {
    Global,
    Project,
}

impl LockKind {
    /// The version Atlas writes for this kind of lock.
    #[must_use]
    pub const fn version(self) -> u64 {
        match self {
            Self::Global => GLOBAL_LOCK_VERSION,
            Self::Project => PROJECT_LOCK_VERSION,
        }
    }
}

/// The global lock's path, following `XDG_STATE_HOME` as the CLI does.
#[must_use]
pub fn global_lock_path(home: &Path, xdg_state_home: Option<&Path>) -> PathBuf {
    match xdg_state_home.filter(|p| !p.as_os_str().is_empty()) {
        Some(state) => state.join("skills").join(GLOBAL_LOCK_FILE),
        None => home.join(".agents").join(GLOBAL_LOCK_FILE),
    }
}

/// A project's lock path.
#[must_use]
pub fn project_lock_path(repo_root: &Path) -> PathBuf {
    repo_root.join(PROJECT_LOCK_FILE)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LockError {
    #[error("the lock file is larger than 4 MB")]
    TooBig,
    #[error("the lock file is a link inside the repo")]
    Link,
    #[error("the lock file isn't valid: {0}")]
    Invalid(String),
    #[error("the lock file has version {0}, older than the skills CLI writes")]
    OldVersion(u64),
    #[error("{0}")]
    Io(String),
}

/// The version a skill's local copy was installed from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Baseline {
    /// The folder's git tree SHA (global lock, `skillFolderHash`).
    Tree(String),
    /// [`computed_hash`] of the folder's files (project lock, `computedHash`).
    Content(String),
}

/// One skill's record, with the fields Atlas uses. Strings are as the file has them;
/// [`Record::github`] is the checked view to fetch from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// `owner/repo` for GitHub; something else for other source types.
    pub source: String,
    /// `github`, `local`, `node_modules`, `well-known`, ...
    pub source_type: String,
    pub source_url: Option<String>,
    /// Where `SKILL.md` is in the source repo, such as `skills/pdf/SKILL.md`.
    pub skill_path: Option<String>,
    /// The branch or tag it was installed from; the default branch when absent.
    pub git_ref: Option<String>,
    /// Global lock: the installed folder's git tree SHA.
    pub skill_folder_hash: Option<String>,
    /// Project lock: the installed files' content hash.
    pub computed_hash: Option<String>,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
}

/// A GitHub source, every part checked against strict patterns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubSource {
    pub owner: String,
    pub repo: String,
    /// The skill's folder in the repo (`skillPath` without `SKILL.md`); `Some("")` is the
    /// repo's top; `None` when the record doesn't say (older records).
    pub folder: Option<String>,
    pub git_ref: Option<String>,
}

impl GitHubSource {
    /// `owner/repo`.
    #[must_use]
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

impl Record {
    fn from_json(entry: &Value) -> Option<Self> {
        let o = entry.as_object()?;
        let text = |k: &str| {
            o.get(k)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            source: o.get("source")?.as_str()?.to_owned(),
            source_type: o.get("sourceType")?.as_str()?.to_owned(),
            source_url: text("sourceUrl"),
            skill_path: text("skillPath"),
            git_ref: text("ref"),
            skill_folder_hash: text("skillFolderHash"),
            computed_hash: text("computedHash"),
            installed_at: text("installedAt"),
            updated_at: text("updatedAt"),
        })
    }

    /// The source on github.com, or `None` for any other kind of source or anything that
    /// fails the checks (an owner of GitHub's account characters, a repo `[A-Za-z0-9_.-]`
    /// once one `.git` is dropped and never `.` or `..`, a `sourceUrl` on github.com, a
    /// plain relative `skillPath`, a ref of safe characters).
    #[must_use]
    pub fn github(&self) -> Option<GitHubSource> {
        if self.source_type != "github" {
            return None;
        }
        let (owner, repo) = self.source.split_once('/')?;
        // Dropped before checking, so `x/...git` can't become `..` in a URL.
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if !is_owner(owner) || !is_repo(repo) {
            return None;
        }
        if let Some(url) = &self.source_url {
            let lower = url.to_ascii_lowercase();
            if !(lower.starts_with("https://github.com/") || lower.starts_with("git@github.com:")) {
                return None;
            }
        }
        let folder = match &self.skill_path {
            None => None,
            Some(p) => Some(folder_of(p)?),
        };
        if let Some(r) = &self.git_ref
            && !is_ref(r)
        {
            return None;
        }
        Some(GitHubSource {
            owner: owner.to_owned(),
            repo: repo.to_owned(),
            folder,
            git_ref: self.git_ref.clone(),
        })
    }

    /// The installed version this record names, in the form its kind of lock keeps.
    #[must_use]
    pub fn baseline(&self, kind: LockKind) -> Option<Baseline> {
        match kind {
            LockKind::Global => self
                .skill_folder_hash
                .as_ref()
                .filter(|h| is_hex(h, 40))
                .map(|h| Baseline::Tree(h.clone())),
            LockKind::Project => self
                .computed_hash
                .as_ref()
                .filter(|h| is_hex(h, 64))
                .map(|h| Baseline::Content(h.clone())),
        }
    }
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// A GitHub account name: letters, digits and hyphens (and the underscore of managed
/// accounts), at most 39, not starting with a hyphen.
fn is_owner(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 39
        && !s.starts_with('-')
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}

/// A GitHub repository name: `[A-Za-z0-9_.-]`, at most 100, never `.` or `..`.
fn is_repo(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// A branch or tag name that stays one safe URL segment once encoded.
#[must_use]
pub fn is_ref(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 200
        && !s.contains("..")
        && !s.contains("//")
        && !s.starts_with(['-', '/'])
        && !s.ends_with('/')
        && s.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'/' | b'@' | b'+' | b'-')
        })
}

/// A relative path inside a folder: `/`-separated, no empty, `.` or `..` part, no
/// backslash, NUL or control character, not absolute. Used for every path that comes from
/// a lock or from GitHub before it is joined to a folder.
#[must_use]
pub fn is_safe_rel(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.bytes().any(|b| b == b'\\' || b < 0x20 || b == 0x7f)
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// `skills/pdf/SKILL.md` → `skills/pdf`; `SKILL.md` → the top (`""`).
fn folder_of(skill_path: &str) -> Option<String> {
    let p = skill_path.trim_matches('/');
    let folder = if p.eq_ignore_ascii_case("skill.md") {
        ""
    } else if p.len() > 9 && p[p.len() - 9..].eq_ignore_ascii_case("/skill.md") {
        &p[..p.len() - 9]
    } else {
        p
    };
    (folder.is_empty() || is_safe_rel(folder)).then(|| folder.to_owned())
}

/// A lock file as read: its usable records and what a rewrite needs to keep its shape.
#[derive(Clone, Debug)]
pub struct LockFile {
    pub path: PathBuf,
    pub kind: LockKind,
    pub version: u64,
    /// Records by skill name. Entries without a string `source` and `sourceType` are left
    /// out here but kept in a rewrite.
    pub records: BTreeMap<String, Record>,
    data: Map<String, Value>,
    indent: String,
    trailing_newline: bool,
    sorted: bool,
}

impl LockFile {
    /// Atlas can rewrite it: the version is the one the CLI writes for this kind.
    #[must_use]
    pub fn writable(&self) -> bool {
        self.version == self.kind.version()
    }

    /// The file's new text with `name`'s record based on `baseline`, the way `npx skills
    /// update` leaves it: a global record gets `skillFolderHash` and `updatedAt` (`now`,
    /// an ISO time), a project record `computedHash`. Everything else stays as it was.
    /// `None` when the file isn't writable, the record is gone or names another source, or
    /// the baseline is the other lock's kind.
    #[must_use]
    pub fn with_baseline(
        &self,
        name: &str,
        source: &str,
        baseline: &Baseline,
        now: &str,
    ) -> Option<String> {
        if !self.writable() || self.records.get(name)?.source != source {
            return None;
        }
        let mut data = self.data.clone();
        let entry = data
            .get_mut("skills")?
            .as_object_mut()?
            .get_mut(name)?
            .as_object_mut()?;
        match (self.kind, baseline) {
            (LockKind::Global, Baseline::Tree(sha)) => {
                entry.insert("skillFolderHash".to_owned(), Value::String(sha.clone()));
                entry.insert("updatedAt".to_owned(), Value::String(now.to_owned()));
            }
            (LockKind::Project, Baseline::Content(hash)) => {
                entry.insert("computedHash".to_owned(), Value::String(hash.clone()));
            }
            _ => return None,
        }
        Some(self.render(data))
    }

    fn render(&self, mut data: Map<String, Value>) -> String {
        if self.kind == LockKind::Project
            && self.sorted
            && let Some(Value::Object(skills)) = data.get_mut("skills")
        {
            let mut entries: Vec<(String, Value)> = std::mem::take(skills).into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            skills.extend(entries);
        }
        let mut out = Vec::new();
        let formatter = serde_json::ser::PrettyFormatter::with_indent(self.indent.as_bytes());
        let mut ser = serde_json::Serializer::with_formatter(&mut out, formatter);
        let _ = serde::Serialize::serialize(&Value::Object(data), &mut ser);
        let mut text = String::from_utf8(out).unwrap_or_default();
        if self.trailing_newline {
            text.push('\n');
        }
        text
    }
}

/// Reads a lock. `Ok(None)` when there is no file. A project lock that is a link is
/// refused (it lives in a repo); the global lock may be one (it is the owner's home).
///
/// # Errors
/// Too big, a link in a repo, not valid JSON in the CLI's shape, an older version, or
/// unreadable.
pub fn read_lock(path: &Path, kind: LockKind) -> Result<Option<LockFile>, LockError> {
    let meta = match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(LockError::Io(e.to_string())),
        Ok(m) => m,
    };
    if meta.file_type().is_symlink() && kind == LockKind::Project {
        return Err(LockError::Link);
    }
    let file = std::fs::File::open(path).map_err(|e| LockError::Io(e.to_string()))?;
    if !file.metadata().is_ok_and(|m| m.is_file()) {
        return Err(LockError::Invalid("not a file".to_owned()));
    }
    let mut text = String::new();
    file.take(MAX_LOCK_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|e| LockError::Io(e.to_string()))?;
    if text.len() as u64 > MAX_LOCK_BYTES {
        return Err(LockError::TooBig);
    }
    parse_lock(&text, path, kind).map(Some)
}

/// Parses a lock's text.
///
/// # Errors
/// Not a JSON object with a numeric `version` and an object `skills`, or an older version.
pub fn parse_lock(text: &str, path: &Path, kind: LockKind) -> Result<LockFile, LockError> {
    let value: Value = serde_json::from_str(text).map_err(|e| LockError::Invalid(e.to_string()))?;
    let Value::Object(data) = value else {
        return Err(LockError::Invalid("not an object".to_owned()));
    };
    let version = data
        .get("version")
        .and_then(Value::as_u64)
        .ok_or_else(|| LockError::Invalid("no version".to_owned()))?;
    let skills = data
        .get("skills")
        .and_then(Value::as_object)
        .ok_or_else(|| LockError::Invalid("no skills".to_owned()))?;
    if version < kind.version() {
        return Err(LockError::OldVersion(version));
    }
    let names: Vec<&String> = skills.keys().collect();
    let sorted = names.windows(2).all(|w| w[0] <= w[1]);
    let records = skills
        .iter()
        .filter_map(|(name, entry)| Some((name.clone(), Record::from_json(entry)?)))
        .collect();
    let indent = text
        .lines()
        .nth(1)
        .map(|l| {
            l.chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "  ".to_owned());
    Ok(LockFile {
        path: path.to_path_buf(),
        kind,
        version,
        records,
        data,
        indent,
        trailing_newline: text.ends_with('\n'),
        sorted,
    })
}

// ---------------------------------------------------------------- hashes

/// Folders the CLI never installs, and litter running a skill leaves behind.
const SKIPPED_DIRS: [&str; 4] = [".git", "node_modules", "__pycache__", "__pypackages__"];
/// Files the CLI never installs (`metadata.json`), and litter macOS leaves.
const SKIPPED_FILES: [&str; 2] = ["metadata.json", ".DS_Store"];

/// Whether comparisons between a local copy and its source leave this path out.
#[must_use]
pub fn is_ignored_path(path: &str) -> bool {
    let mut parts: Vec<&str> = path.split('/').collect();
    let name = parts.pop().unwrap_or("");
    SKIPPED_FILES.contains(&name) || parts.iter().any(|p| SKIPPED_DIRS.contains(p))
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

/// What git names these bytes as a file (its blob SHA).
#[must_use]
pub fn git_blob_sha(bytes: &[u8]) -> String {
    hex(&object_sha("blob", bytes))
}

/// One entry of a folder by its git blob SHA.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeItem {
    /// Relative to the folder, `/`-separated.
    pub path: String,
    /// `100644`, `100755` or `120000`.
    pub mode: String,
    pub sha: String,
}

enum Node<'a> {
    Dir(BTreeMap<&'a str, Node<'a>>),
    Item(&'a TreeItem),
}

fn encode(node: &BTreeMap<&str, Node<'_>>) -> Vec<u8> {
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = node
        .iter()
        .map(|(name, child)| match child {
            Node::Dir(sub) => {
                let mut head = format!("40000 {name}\0").into_bytes();
                head.extend_from_slice(&object_sha("tree", &encode(sub)));
                // Git sorts a folder as if its name ended with a slash.
                (format!("{name}/").into_bytes(), head)
            }
            Node::Item(item) => {
                let mut head = format!("{} {name}\0", item.mode).into_bytes();
                let raw: Vec<u8> = (0..item.sha.len() / 2)
                    .filter_map(|i| u8::from_str_radix(&item.sha[i * 2..i * 2 + 2], 16).ok())
                    .collect();
                head.extend_from_slice(&raw);
                (name.as_bytes().to_vec(), head)
            }
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries.into_iter().flat_map(|(_, head)| head).collect()
}

/// The tree SHA git gives a folder holding these items: what `git write-tree` prints and
/// what GitHub lists as the folder's `sha`.
#[must_use]
pub fn git_tree_sha(items: &[TreeItem]) -> String {
    let mut root: BTreeMap<&str, Node<'_>> = BTreeMap::new();
    for item in items {
        let mut parts: Vec<&str> = item.path.split('/').collect();
        let name = parts.pop().unwrap_or("");
        let mut node = &mut root;
        for part in parts {
            let entry = node
                .entry(part)
                .or_insert_with(|| Node::Dir(BTreeMap::new()));
            if let Node::Item(_) = entry {
                *entry = Node::Dir(BTreeMap::new());
            }
            let Node::Dir(sub) = entry else {
                unreachable!()
            };
            node = sub;
        }
        node.insert(name, Node::Item(item));
    }
    hex(&object_sha("tree", &encode(&root)))
}

/// The CLI's `computeSkillFolderHash`: SHA-256 over each regular file's path and bytes,
/// ordered as JavaScript's `localeCompare` orders the paths, leaving out `.git` and
/// `node_modules`. `files` are `(path relative to the folder, bytes)`.
#[must_use]
pub fn computed_hash(files: &[(&str, &[u8])]) -> String {
    let mut kept: Vec<&(&str, &[u8])> = files
        .iter()
        .filter(|(p, _)| {
            let mut parts: Vec<&str> = p.split('/').collect();
            parts.pop();
            !parts.iter().any(|d| *d == ".git" || *d == "node_modules")
        })
        .collect();
    kept.sort_by(|a, b| locale_cmp(a.0, b.0));
    let mut h = <sha2::Sha256 as sha2::Digest>::new();
    for (p, bytes) in kept {
        sha2::Digest::update(&mut h, p.as_bytes());
        sha2::Digest::update(&mut h, bytes);
    }
    hex(&sha2::Digest::finalize(h))
}

/// ICU's root collation for ASCII, as `localeCompare` without a locale applies it:
/// punctuation before digits before letters, letters compared without case first, then
/// lower case before upper case. Non-ASCII text falls back to code point order after
/// ASCII, which may differ from ICU for such names (rare in skill folders).
fn locale_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    const PUNCT: &[u8] = b"\t\n\r _-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    fn primary(c: char) -> (u8, u32) {
        if c.is_ascii() {
            let b = c as u8;
            if let Some(i) = PUNCT.iter().position(|p| *p == b) {
                return (0, u32::try_from(i).unwrap_or(0));
            }
            if b.is_ascii_digit() {
                return (1, u32::from(b));
            }
            if b.is_ascii_alphabetic() {
                return (2, u32::from(b.to_ascii_lowercase()));
            }
            return (0, 100 + u32::from(b));
        }
        (3, u32::from(c))
    }
    let pa: Vec<(u8, u32)> = a.chars().map(primary).collect();
    let pb: Vec<(u8, u32)> = b.chars().map(primary).collect();
    pa.cmp(&pb).then_with(|| {
        // Lower case first.
        let tert = |s: &str| -> Vec<u8> {
            s.chars()
                .map(|c| u8::from(c.is_ascii_uppercase()))
                .collect()
        };
        tert(a).cmp(&tert(b))
    })
}

// ---------------------------------------------------------------- moving records

// The skill library carries a skill's record when the skill moves between Global and a
// project, and drops it when the skill is deleted (only in locks inside repos Atlas
// commits). Rewrites keep every other record, unknown fields and the file's formatting.

impl LockFile {
    /// An empty lock of `kind` at `path`, as the CLI first writes it.
    #[must_use]
    pub fn fresh(path: &Path, kind: LockKind) -> Self {
        let mut data = Map::new();
        data.insert("version".to_owned(), Value::from(kind.version()));
        data.insert("skills".to_owned(), Value::Object(Map::new()));
        if kind == LockKind::Global {
            data.insert("dismissed".to_owned(), Value::Object(Map::new()));
        }
        Self {
            path: path.to_path_buf(),
            kind,
            version: kind.version(),
            records: BTreeMap::new(),
            data,
            indent: "  ".to_owned(),
            trailing_newline: kind == LockKind::Project,
            sorted: true,
        }
    }

    /// `name`'s record exactly as the file has it, unknown fields included.
    #[must_use]
    pub fn raw(&self, name: &str) -> Option<&Value> {
        self.data.get("skills")?.as_object()?.get(name)
    }

    /// Takes `name`'s record out, returning it whole.
    pub fn remove(&mut self, name: &str) -> Option<Value> {
        self.records.remove(name);
        self.data
            .get_mut("skills")?
            .as_object_mut()?
            .shift_remove(name)
    }

    /// Sets `name`'s record to `entry`, kept whole in the file.
    pub fn insert(&mut self, name: &str, entry: Value) {
        if let Some(record) = Record::from_json(&entry) {
            self.records.insert(name.to_owned(), record);
        }
        if let Some(skills) = self.data.get_mut("skills").and_then(Value::as_object_mut) {
            skills.insert(name.to_owned(), entry);
        }
    }

    /// The file's text as it now stands, in its own formatting.
    #[must_use]
    pub fn text(&self) -> String {
        self.render(self.data.clone())
    }
}

/// Both hashes the CLI keeps for a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderHashes {
    /// Git's tree SHA, which GitHub reports and the global lock keeps (`skillFolderHash`).
    pub tree_sha: String,
    /// [`computed_hash`], which a project lock keeps (`computedHash`).
    pub computed_hash: String,
}

/// The hashes of a skill folder on disk, or `None` when it can't be read or holds
/// anything but plain files and folders (its tree SHA would then be unknowable here).
#[must_use]
pub fn hash_folder(folder: &Path) -> Option<FolderHashes> {
    let tree = crate::library::read_tree(folder).ok()?;
    tree.exact_digest()?;
    let files: Vec<(String, &[u8], bool)> = tree
        .files
        .iter()
        .map(|f| {
            (
                f.rel.to_string_lossy().replace('\\', "/"),
                f.bytes.as_slice(),
                f.executable,
            )
        })
        .collect();
    let items: Vec<TreeItem> = files
        .iter()
        .map(|(path, bytes, exec)| TreeItem {
            path: path.clone(),
            mode: if *exec { "100755" } else { "100644" }.to_owned(),
            sha: git_blob_sha(bytes),
        })
        .collect();
    let plain: Vec<(&str, &[u8])> = files.iter().map(|(p, b, _)| (p.as_str(), *b)).collect();
    Some(FolderHashes {
        tree_sha: git_tree_sha(&items),
        computed_hash: computed_hash(&plain),
    })
}

/// `entry` (a record of a `from` lock) in the shape a `to` lock keeps, or `None` when it
/// can't be carried: a local source means nothing elsewhere, a global record needs the
/// URL the CLI reinstalls from, and a project record needs the folder's `computedHash`,
/// known only when the folder is exactly the tree the global record names (`hashes`).
/// Follows T3 Code's `SkillLockFiles.convertRecord`.
#[must_use]
pub fn convert_record(
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
    if from == to {
        return Some(entry.clone());
    }
    let text = |k: &str| e.get(k).and_then(Value::as_str);
    let mut out = Map::new();
    let mut put = |k: &str, v: Option<&str>| {
        if let Some(v) = v {
            out.insert(k.to_owned(), Value::from(v));
        }
    };
    let default_url = format!("https://github.com/{source}.git");
    if from == LockKind::Project {
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
    } else {
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
    Some(Value::Object(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL_FILE: &str = "---\nname: db-migrations\ndescription: Plan and run database migrations.\n---\n\n# Migrations\n";

    /// From T3 Code's `SkillLockFiles.test.ts`: what the CLI's `computeSkillFolderHash`
    /// and `git write-tree` give this folder.
    #[test]
    fn hashes_match_the_cli_and_git() {
        let files: [(&str, &[u8]); 3] = [
            ("SKILL.md", SKILL_FILE.as_bytes()),
            ("references/x.md", b"Read this first.\n"),
            ("scripts/run.sh", b"#!/bin/sh\necho migrate\n"),
        ];
        assert_eq!(
            computed_hash(&files),
            "a7f77818fb1962dfbb40da69550e2c9c0c035e97a11012946465db99bad816c0"
        );
        let mut with_modules = files.to_vec();
        with_modules.push(("node_modules/ignored.js", b"ignored\n"));
        assert_eq!(computed_hash(&with_modules), computed_hash(&files));
        let items: Vec<TreeItem> = files
            .iter()
            .map(|(p, b)| TreeItem {
                path: (*p).to_owned(),
                mode: if p.starts_with("scripts/") {
                    "100755"
                } else {
                    "100644"
                }
                .to_owned(),
                sha: git_blob_sha(b),
            })
            .collect();
        assert_eq!(
            git_tree_sha(&items),
            "18071366eef226103eef569e462d6a3e8e11fac7"
        );
        assert_eq!(
            git_blob_sha(b""),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
    }

    #[test]
    fn locale_order_puts_case_last() {
        let mut v = vec![
            "SKILL.md",
            "scripts/run.sh",
            "references/x.md",
            "a-b",
            "a_b",
            "a1",
            "Ab",
            "ab",
        ];
        v.sort_by(|a, b| locale_cmp(a, b));
        assert_eq!(
            v,
            [
                "a_b",
                "a-b",
                "a1",
                "ab",
                "Ab",
                "references/x.md",
                "scripts/run.sh",
                "SKILL.md"
            ]
        );
    }

    fn project_text(skills: &Value) -> String {
        format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::json!({"version": 1, "skills": skills}))
                .unwrap()
        )
    }

    fn global_entry() -> Value {
        serde_json::json!({
            "source": "acme/skills",
            "sourceType": "github",
            "sourceUrl": "https://github.com/acme/skills.git",
            "skillPath": "skills/db-migrations/SKILL.md",
            "skillFolderHash": "18071366eef226103eef569e462d6a3e8e11fac7",
            "installedAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-02T00:00:00.000Z"
        })
    }

    #[test]
    fn reads_github_records_from_both_locks() {
        let project = project_text(&serde_json::json!({
            "db-migrations": {
                "source": "acme/skills", "sourceType": "github",
                "skillPath": "skills/db-migrations/SKILL.md", "computedHash": "0".repeat(64)
            },
            "from-npm": {"source": "left-pad", "sourceType": "node_modules", "computedHash": "x"},
            "from-disk": {"source": "./skills/local", "sourceType": "local", "computedHash": "x"},
        }));
        let lock = parse_lock(&project, Path::new("skills-lock.json"), LockKind::Project).unwrap();
        assert!(lock.writable());
        let gh: Vec<_> = lock
            .records
            .iter()
            .filter_map(|(n, r)| Some((n.as_str(), r.github()?)))
            .collect();
        assert_eq!(gh.len(), 1);
        assert_eq!(gh[0].0, "db-migrations");
        assert_eq!(gh[0].1.slug(), "acme/skills");
        assert_eq!(gh[0].1.folder.as_deref(), Some("skills/db-migrations"));
        assert_eq!(
            lock.records["db-migrations"].baseline(LockKind::Project),
            Some(Baseline::Content("0".repeat(64)))
        );

        let global = serde_json::to_string_pretty(&serde_json::json!({
            "version": 3,
            "skills": {
                "global-one": global_entry(),
                "odd-source": {"source": "not a repo", "sourceType": "github"},
                "elsewhere": {"source": "acme/x", "sourceType": "github", "sourceUrl": "https://gitlab.com/acme/x.git"},
                "untracked": {"source": "acme/x", "sourceType": "github", "skillFolderHash": ""},
            },
            "dismissed": {"findSkillsPrompt": true}
        }))
        .unwrap();
        let lock = parse_lock(&global, Path::new("g"), LockKind::Global).unwrap();
        let rec = &lock.records["global-one"];
        assert_eq!(
            rec.baseline(LockKind::Global),
            Some(Baseline::Tree(
                "18071366eef226103eef569e462d6a3e8e11fac7".to_owned()
            ))
        );
        assert!(lock.records["odd-source"].github().is_none());
        assert!(lock.records["elsewhere"].github().is_none());
        assert_eq!(lock.records["untracked"].baseline(LockKind::Global), None);
        assert_eq!(lock.records["untracked"].github().unwrap().folder, None);
    }

    #[test]
    fn refuses_what_the_cli_would_treat_as_empty() {
        for bad in [
            "<<<<<<< HEAD\n{\"version\":1}\n>>>>>>>\n",
            "{\"version\":3}",
            "[]",
        ] {
            assert!(matches!(
                parse_lock(bad, Path::new("x"), LockKind::Global),
                Err(LockError::Invalid(_))
            ));
        }
        assert_eq!(
            parse_lock(
                "{\"version\":2,\"skills\":{}}",
                Path::new("x"),
                LockKind::Global
            )
            .unwrap_err(),
            LockError::OldVersion(2)
        );
        // A newer version is read but never written.
        let newer = parse_lock(
            "{\"version\":4,\"skills\":{\"a\":{\"source\":\"o/r\",\"sourceType\":\"github\"}}}",
            Path::new("x"),
            LockKind::Global,
        )
        .unwrap();
        assert!(!newer.writable());
        assert!(
            newer
                .with_baseline("a", "o/r", &Baseline::Tree("a".repeat(40)), "now")
                .is_none()
        );
    }

    #[test]
    fn strict_patterns_for_sources() {
        let rec = |source: &str, path: Option<&str>, r: Option<&str>| Record {
            source: source.to_owned(),
            source_type: "github".to_owned(),
            source_url: None,
            skill_path: path.map(str::to_owned),
            git_ref: r.map(str::to_owned),
            skill_folder_hash: None,
            computed_hash: None,
            installed_at: None,
            updated_at: None,
        };
        assert!(
            rec("acme/skills", Some("SKILL.md"), Some("v1.2"))
                .github()
                .is_some()
        );
        assert_eq!(
            rec("acme/skills", Some("SKILL.md"), None)
                .github()
                .unwrap()
                .folder
                .as_deref(),
            Some("")
        );
        for bad in [
            "acme",
            "../x",
            "acme/..",
            "a/b/c",
            "a b/c",
            "acme/sk%2e",
            // One `.git` is dropped first, so these would name `..` or nothing.
            "acme/...git",
            "acme/.git",
            "acme/..git",
            "./skills",
            "a.b/skills",
            "-acme/skills",
        ] {
            assert!(rec(bad, None, None).github().is_none(), "{bad}");
        }
        assert_eq!(
            rec("acme/skills.git", None, None).github().unwrap().repo,
            "skills"
        );
        assert_eq!(
            rec("acme/skills.git.git", None, None)
                .github()
                .unwrap()
                .repo,
            "skills.git"
        );
        for bad in [
            "../SKILL.md",
            "a/../../SKILL.md",
            "a\\b/SKILL.md",
            "a/./SKILL.md",
            "a//b",
        ] {
            assert!(rec("o/r", Some(bad), None).github().is_none(), "{bad}");
        }
        for bad in ["-x", "a..b", "a b", "/x", "x/", "a?b", "a#b", "a%2f"] {
            assert!(rec("o/r", None, Some(bad)).github().is_none(), "{bad}");
        }
        assert!(is_safe_rel("scripts/run.sh"));
        for bad in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a/./b",
            "a//b",
            "a\\b",
            "a\0b",
            "a\nb",
        ] {
            assert!(!is_safe_rel(bad), "{bad:?}");
        }
    }

    #[test]
    fn rewrites_keep_the_files_shape() {
        // Global: 4-space indent, no trailing newline, key order kept.
        let text = serde_json::to_string_pretty(&serde_json::json!({
            "version": 3,
            "skills": {"zeta": global_entry(), "alpha": global_entry()},
            "dismissed": {"findSkillsPrompt": true}
        }))
        .unwrap()
        .replace("\n  ", "\n    ")
        .replace("\n      ", "\n        ");
        let lock = parse_lock(&text, Path::new("g"), LockKind::Global).unwrap();
        let new_sha = "b".repeat(40);
        let out = lock
            .with_baseline(
                "zeta",
                "acme/skills",
                &Baseline::Tree(new_sha.clone()),
                "2026-11-27T00:00:00.000Z",
            )
            .unwrap();
        assert!(out.starts_with("{\n    \"version\": 3"), "{out}");
        assert!(!out.ends_with('\n'));
        let back: Value = serde_json::from_str(&out).unwrap();
        let keys: Vec<&String> = back["skills"].as_object().unwrap().keys().collect();
        assert_eq!(keys, ["zeta", "alpha"]);
        assert_eq!(back["skills"]["zeta"]["skillFolderHash"], new_sha.as_str());
        assert_eq!(
            back["skills"]["zeta"]["updatedAt"],
            "2026-11-27T00:00:00.000Z"
        );
        assert_eq!(back["skills"]["alpha"], global_entry());
        assert_eq!(back["dismissed"]["findSkillsPrompt"], true);
        // Another source, the wrong kind, or a missing record: nothing.
        assert!(
            lock.with_baseline("zeta", "other/repo", &Baseline::Tree(new_sha.clone()), "t")
                .is_none()
        );
        assert!(
            lock.with_baseline(
                "zeta",
                "acme/skills",
                &Baseline::Content("c".repeat(64)),
                "t"
            )
            .is_none()
        );
        assert!(
            lock.with_baseline("gone", "acme/skills", &Baseline::Tree(new_sha), "t")
                .is_none()
        );

        // Project: sorted, trailing newline.
        let text = project_text(&serde_json::json!({
            "a": {"source": "o/r", "sourceType": "github", "computedHash": "0".repeat(64)},
            "b": {"source": "o/r", "sourceType": "github", "computedHash": "1".repeat(64)},
        }));
        let lock = parse_lock(&text, Path::new("p"), LockKind::Project).unwrap();
        let out = lock
            .with_baseline("b", "o/r", &Baseline::Content("f".repeat(64)), "t")
            .unwrap();
        assert_eq!(out, text.replace(&"1".repeat(64), &"f".repeat(64)));
    }

    #[test]
    fn reads_files_and_follows_xdg() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            global_lock_path(tmp.path(), None),
            tmp.path().join(".agents/.skill-lock.json")
        );
        assert_eq!(
            global_lock_path(tmp.path(), Some(&tmp.path().join("state"))),
            tmp.path().join("state/skills/.skill-lock.json")
        );
        let p = project_lock_path(tmp.path());
        assert!(read_lock(&p, LockKind::Project).unwrap().is_none());
        std::fs::write(
            tmp.path().join("real.json"),
            "{\"version\":3,\"skills\":{}}",
        )
        .unwrap();
        std::os::unix::fs::symlink(tmp.path().join("real.json"), &p).unwrap();
        assert_eq!(
            read_lock(&p, LockKind::Project).unwrap_err(),
            LockError::Link
        );
        assert!(read_lock(&p, LockKind::Global).unwrap().is_some());
    }

    #[test]
    fn ignored_paths() {
        for p in [
            "metadata.json",
            "a/.DS_Store",
            "node_modules/x.js",
            "s/__pycache__/x.pyc",
        ] {
            assert!(is_ignored_path(p), "{p}");
        }
        for p in ["SKILL.md", "scripts/metadata.json.bak", "x/node_modules.md"] {
            assert!(!is_ignored_path(p), "{p}");
        }
    }

    #[test]
    fn records_move_between_locks_keeping_everything_else() {
        let global = "{\n  \"version\": 3,\n  \"skills\": {\n    \"pdf\": {\n      \"source\": \"acme/tools\",\n      \"sourceType\": \"github\",\n      \"sourceUrl\": \"https://github.com/acme/tools.git\",\n      \"skillFolderHash\": \"\",\n      \"extra\": 1\n    },\n    \"other\": {\n      \"source\": \"a/b\",\n      \"sourceType\": \"github\"\n    }\n  },\n  \"dismissed\": {},\n  \"future\": true\n}";
        let mut lock = parse_lock(global, Path::new("g"), LockKind::Global).unwrap();
        let pdf = lock.remove("pdf").unwrap();
        assert_eq!(pdf["extra"], 1);
        assert!(!lock.records.contains_key("pdf"));
        let text = lock.text();
        assert!(
            text.contains("\"future\": true") && text.contains("\"other\""),
            "{text}"
        );
        assert!(!text.ends_with('\n'));
        lock.insert("pdf", pdf.clone());
        assert!(lock.records.contains_key("pdf"));

        // Untracked global records have no known hash, so they can't become project ones.
        assert!(convert_record(&pdf, LockKind::Global, LockKind::Project, None, "now").is_none());
        let project = serde_json::json!({"source": "acme/tools", "sourceType": "github", "computedHash": "ab"});
        let back =
            convert_record(&project, LockKind::Project, LockKind::Global, None, "now").unwrap();
        assert_eq!(back["sourceUrl"], "https://github.com/acme/tools.git");
        assert_eq!(back["skillFolderHash"], "");
        let mut tracked = pdf;
        tracked["skillFolderHash"] = Value::from("abc");
        let hashes = FolderHashes {
            tree_sha: "abc".to_owned(),
            computed_hash: "def".to_owned(),
        };
        let to_project = convert_record(
            &tracked,
            LockKind::Global,
            LockKind::Project,
            Some(&hashes),
            "now",
        )
        .unwrap();
        assert_eq!(to_project["computedHash"], "def");
        assert!(to_project.get("sourceUrl").is_none());
        let local = serde_json::json!({"source": "./x", "sourceType": "local"});
        assert!(convert_record(&local, LockKind::Global, LockKind::Global, None, "now").is_none());

        let mut fresh = LockFile::fresh(Path::new("p"), LockKind::Project);
        fresh.insert("zeta", project.clone());
        fresh.insert("alpha", project);
        let text = fresh.text();
        assert!(
            text.ends_with("}\n") && text.find("alpha") < text.find("zeta"),
            "{text}"
        );
    }

    #[test]
    fn a_folder_hashes_as_git_does() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("x/scripts")).unwrap();
        std::fs::write(root.join("x/SKILL.md"), "---\nname: x\n---\n").unwrap();
        std::fs::write(root.join("x/scripts/run.sh"), "echo hi\n").unwrap();
        std::fs::set_permissions(
            root.join("x/scripts/run.sh"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
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
        std::os::unix::fs::symlink("SKILL.md", root.join("x/link")).unwrap();
        assert!(hash_folder(&root.join("x")).is_none());
    }
}
