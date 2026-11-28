//! What sits in this login's agent folders outside Atlas, and whether Atlas can bring it in
//! (decision 0027, `documents/skills.md` "Bringing things in").
//!
//! [`scan`] looks at every harness's skill folders, the skills that come with an agent or
//! one of its plugins, the hooks the harness scan found and Claude Code's per-repo memory,
//! and says for each item what it is. The rules are pure functions of paths and text
//! ([`place_of`], [`hook_owner`]) so they are tested without a disk. [`hook_manifest`],
//! [`hook_script`] and [`notes_index`] give the files an import writes.
//!
//! Only the login's home is ever a source: a link into a tool's install (`/usr/...`, a
//! tool's own dot-folder, a package store) is listed as "installed by a tool" and never
//! copied, and agents' own built-in and plugin skills are only listed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Component, Path, PathBuf};

use atlas_core::catalog::{
    Catalog, HookEvent, HookManifest, Keep, KeepKind, Placements, SkillHome,
};
use atlas_core::distribution::HarnessView;
use atlas_core::kernel::{HarnessKind, Scope, Slug};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::library;
use crate::paths::Sources;

/// Folders outside the home that tools install into.
pub const TOOL_ROOTS: [&str; 8] = [
    "/usr",
    "/opt",
    "/nix",
    "/snap",
    "/var/lib/flatpak",
    "/Applications",
    "/Library",
    "/System",
];

/// The file in a project's store memory folder that records where its notes came from.
pub const NOTES_INDEX: &str = "index.md";

/// The biggest JSON file read to list Claude Code's plugins.
const MAX_JSON_BYTES: u64 = 1024 * 1024;
/// The biggest note compared with the store's copy.
const MAX_NOTE_BYTES: u64 = 1024 * 1024;
/// Entries listed from any one folder.
const MAX_ENTRIES: usize = 500;

/// Where tools live, for telling a tool's folder from the owner's.
#[derive(Clone, Debug)]
pub struct Rules {
    pub tool_roots: Vec<PathBuf>,
}

impl Default for Rules {
    fn default() -> Self {
        Self {
            tool_roots: TOOL_ROOTS.iter().map(PathBuf::from).collect(),
        }
    }
}

// ---------------------------------------------------------------- where a path is

/// Who a folder belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// The owner's own files in their home: Atlas may copy from here.
    Owner,
    /// A folder a tool installed or keeps, named when that's recognisable.
    Tool(Option<String>),
    /// Inside a Claude Code plugin.
    ClaudePlugin,
    /// Codex's own system skills.
    CodexSystem,
}

/// Top-level folders in the home that belong to agents (from the layouts), and the
/// agents' folders under `.config`. Hosts (T3 Code, Herdr) are tools here.
fn agent_folders() -> (BTreeSet<&'static str>, BTreeSet<&'static str>) {
    let mut top = BTreeSet::new();
    let mut config = BTreeSet::new();
    for l in atlas_harness::layouts() {
        if matches!(l.kind, HarnessKind::T3 | HarnessKind::Herdr) {
            continue;
        }
        let paths = l
            .skill_dirs
            .iter()
            .chain(l.instruction_files)
            .chain(l.hook_config.iter())
            .chain(std::iter::once(&l.marker));
        for p in paths {
            let mut parts = p.split('/');
            match (parts.next(), parts.next()) {
                (Some(".config"), Some(sub)) => {
                    config.insert(sub);
                }
                (Some(".config" | ".local") | None, _) => {}
                (Some(first), _) => {
                    top.insert(first);
                }
            }
        }
    }
    (top, config)
}

/// A tool's name from the folders after a tool root: the first that isn't a generic
/// one (`share`, `lib`, ...), without a Nix store hash.
fn tool_named(rest: &Path) -> Option<String> {
    const GENERIC: [&str; 9] = [
        "share", "lib", "lib64", "libexec", "local", "bin", "etc", "store", "opt",
    ];
    rest.components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .find(|s| !GENERIC.contains(s))
        .map(|s| {
            // /nix/store/<32 chars>-name
            match s.split_once('-') {
                Some((hash, name)) if hash.len() == 32 && !name.is_empty() => name.to_owned(),
                _ => s.trim_start_matches('.').to_owned(),
            }
        })
}

/// Who `path` (absolute; resolve links first) belongs to.
#[must_use]
pub fn place_of(home: &Path, rules: &Rules, path: &Path) -> Place {
    let Ok(rel) = path.strip_prefix(home) else {
        let name = rules
            .tool_roots
            .iter()
            .find_map(|root| path.strip_prefix(root).ok())
            .and_then(tool_named);
        return Place::Tool(name);
    };
    let parts: Vec<&str> = rel
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => s.to_str(),
            _ => None,
        })
        .collect();
    let part = |i: usize| parts.get(i).copied();
    let named = |i: usize| part(i).map(|s| s.trim_start_matches('.').to_owned());
    if parts
        .iter()
        .any(|p| matches!(*p, "node_modules" | ".pnpm-store"))
    {
        return Place::Tool(None);
    }
    match (part(0), part(1), part(2)) {
        (Some(".claude"), Some("plugins"), _) => Place::ClaudePlugin,
        (Some(".codex"), Some("skills"), Some(".system")) => Place::CodexSystem,
        (Some(".local"), Some("share"), _) | (Some("Library"), Some("Application Support"), _) => {
            Place::Tool(named(2))
        }
        (Some(".local"), ..) => Place::Tool(None),
        (Some(".cache" | "Library"), ..) => Place::Tool(named(1)),
        (Some(".config"), sub, _) => {
            let (_, config) = agent_folders();
            if sub.is_some_and(|s| config.contains(s)) {
                Place::Owner
            } else {
                Place::Tool(named(1))
            }
        }
        (Some("go"), Some("pkg"), _) => Place::Tool(Some("go".to_owned())),
        (Some(first), ..) if first.starts_with('.') => {
            let (top, _) = agent_folders();
            if top.contains(first) {
                Place::Owner
            } else {
                Place::Tool(named(0))
            }
        }
        _ => Place::Owner,
    }
}

// ---------------------------------------------------------------- skills

/// What an outside skill entry is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// A real folder in an agent's skills folder.
    OwnFolder,
    /// A link to a folder elsewhere in the home, not in Atlas.
    Linked { target: String },
    /// A link into a tool's install. Never imported.
    Tool {
        tool: Option<String>,
        target: String,
    },
    /// A link whose target is gone. atlasd offers to remove it; the hub only reports it.
    Broken { target: String },
}

impl Origin {
    /// Whether Atlas may copy it into the store.
    #[must_use]
    pub const fn importable(&self) -> bool {
        matches!(self, Self::OwnFolder | Self::Linked { .. })
    }
}

/// An Atlas skill with the same name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clash {
    pub home: SkillHome,
    /// The folders hold exactly the same files (`library::Tree::exact_digest`).
    pub same: bool,
}

/// One skill folder outside Atlas, however many agent folders show it.
#[derive(Clone, Debug)]
pub struct OutsideSkill {
    /// `skill:<first entry>`, the id in requests and in `[keep]`.
    pub key: String,
    pub name: String,
    /// The agent-folder entries that show it, as `~/...` paths.
    pub entries: Vec<String>,
    /// Agents set up here that read one of those entries.
    pub kinds: Vec<HarnessKind>,
    pub origin: Origin,
    /// The real folder (links resolved), when there is one.
    pub real: Option<PathBuf>,
    /// The real folder as `~/...`.
    pub shown_real: String,
    /// A registered project the real folder sits in.
    pub project: Option<Slug>,
    pub has_skill_md: bool,
    /// What a copy would leave out (links, a `.git`, special files), relative to the
    /// folder. atlasd never replaces such an original, so the owner removes it by hand.
    pub left_out: Vec<String>,
    pub clash: Option<Clash>,
    /// The owner chose to leave it outside.
    pub kept: bool,
}

impl OutsideSkill {
    /// Why it can't be brought in as it is, or None when it can.
    #[must_use]
    pub fn blocked(&self) -> Option<String> {
        if !self.origin.importable() {
            return Some(match &self.origin {
                Origin::Broken { .. } => "Broken link".to_owned(),
                Origin::Tool { tool: Some(t), .. } => format!("Installed by {t}"),
                _ => "Installed by a tool".to_owned(),
            });
        }
        if Slug::new(self.name.as_str()).is_err() {
            return Some("Its name isn't lowercase-with-hyphens; rename it first".to_owned());
        }
        if !self.has_skill_md {
            return Some("No SKILL.md".to_owned());
        }
        None
    }
}

/// A skill that came with an agent or a plugin. Listed, never moved or imported.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltIn {
    pub agent: HarnessKind,
    /// As the agent names it: `<plugin>:<folder>` for a Claude Code plugin.
    pub name: String,
    pub source: String,
    pub path: String,
}

// ---------------------------------------------------------------- projects (Tidy up)

/// A real skill folder in a project's `.claude/skills`, which only Claude-style agents read.
#[derive(Clone, Debug)]
pub struct ClaudeOnly {
    pub project: Slug,
    pub name: String,
    /// `.agents/skills/<name>` exists already, so it can't simply move there.
    pub taken: bool,
}

/// A project skill whose name is also a Global skill.
#[derive(Clone, Debug)]
pub struct Duplicate {
    pub project: Slug,
    pub name: Slug,
    pub global: SkillHome,
    pub same: bool,
}

/// A project skill switched off for agents set up here.
#[derive(Clone, Debug)]
pub struct SwitchedOff {
    pub project: Slug,
    pub name: Slug,
    pub agents: Vec<HarnessKind>,
}

// ---------------------------------------------------------------- hooks

/// Who put a hook in a harness's config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookOwner {
    /// Atlas's own entry, `# atlas:<name>`.
    Atlas(String),
    /// A tool's integration, such as Herdr's.
    Tool(String),
    /// The owner, by hand.
    Owner,
}

/// One hook in a harness's config that isn't Atlas's.
#[derive(Clone, Debug)]
pub struct OutsideHook {
    /// `hook:<config>#<fingerprint>`; the command never appears in it.
    pub key: String,
    pub kind: HarnessKind,
    pub config: String,
    /// The harness's own event name.
    pub event: String,
    pub matcher: Option<String>,
    /// The command as the harness has it. Mask before showing.
    pub command: String,
    pub timeout: Option<u64>,
    pub owner: HookOwner,
    pub atlas_event: Option<HookEvent>,
    /// A free folder name for it under `hooks/`.
    pub name: String,
    pub kept: bool,
}

impl OutsideHook {
    /// Why it can't be brought in, or None when it can.
    #[must_use]
    pub fn blocked(&self) -> Option<String> {
        if let HookOwner::Tool(t) = &self.owner {
            return Some(format!("Added by {t}"));
        }
        if self.atlas_event.is_none() {
            return Some(format!("Atlas has no {} event", self.event));
        }
        match self.kind {
            HarnessKind::ClaudeCode | HarnessKind::Codex => None,
            HarnessKind::Grok => Some(
                "Grok Build keeps Atlas's hooks in a file of their own, so this one would still run"
                    .to_owned(),
            ),
            other => Some(format!(
                "Atlas doesn't write {}'s hooks yet",
                other.display_name()
            )),
        }
    }
}

// ---------------------------------------------------------------- memory

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteState {
    /// Not in the store yet.
    New,
    /// The store has it byte for byte.
    Same,
    /// The store has a note of that name with other text.
    Differs,
}

#[derive(Clone, Debug)]
pub struct Note {
    /// `memory:<folder>/<file>`.
    pub key: String,
    pub file: String,
    pub state: NoteState,
    pub kept: bool,
}

/// Claude Code's own notes on one registered project.
#[derive(Clone, Debug)]
pub struct ProjectNotes {
    pub project: Slug,
    /// Its folder, absolute, and as `~/...`.
    pub folder: PathBuf,
    pub shown: String,
    pub notes: Vec<Note>,
    pub problem: Option<String>,
}

/// A Claude Code memory folder for a repo Atlas doesn't know as a project.
#[derive(Clone, Debug)]
pub struct StrayNotes {
    /// The folder's own name under `~/.claude/projects`.
    pub key: String,
    pub notes: usize,
}

/// Everything outside Atlas on this machine, as of one look.
#[derive(Clone, Debug, Default)]
pub struct Outside {
    pub skills: Vec<OutsideSkill>,
    pub built_in: Vec<BuiltIn>,
    pub hooks: Vec<OutsideHook>,
    pub memory: Vec<ProjectNotes>,
    pub stray_memory: Vec<StrayNotes>,
    pub claude_only: Vec<ClaudeOnly>,
    pub duplicates: Vec<Duplicate>,
    pub switched_off: Vec<SwitchedOff>,
}

impl Outside {
    /// Items the owner chose to leave outside.
    #[must_use]
    pub fn kept(&self) -> usize {
        self.skills.iter().filter(|s| s.kept).count()
            + self.hooks.iter().filter(|h| h.kept).count()
            + self
                .memory
                .iter()
                .flat_map(|p| &p.notes)
                .filter(|n| n.kept)
                .count()
    }
}

// ---------------------------------------------------------------- the scan

struct Ctx<'a> {
    sources: &'a Sources,
    rules: &'a Rules,
    /// The home with links resolved, which resolved targets are compared with.
    home: PathBuf,
    keep: Keep,
}

impl Ctx<'_> {
    fn shown(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => self.sources.display(path),
        }
    }

    fn kept(&self, kind: KeepKind, shown: &str) -> bool {
        self.keep.keeps(kind, shown)
    }
}

/// Where a store skill lives on the hub.
#[must_use]
pub fn skill_folder(sources: &Sources, home: &SkillHome, name: &str) -> Option<PathBuf> {
    match home {
        SkillHome::Private => Some(sources.knowledge.join("skills").join(name)),
        SkillHome::Public => sources.public_skills.as_ref().map(|p| p.join(name)),
        SkillHome::Project(p) => sources
            .projects
            .iter()
            .find(|x| &x.name == p)
            .map(|x| x.root.join(".agents/skills").join(name)),
    }
}

/// Whether two folders hold exactly the same files. Unreadable, or holding anything a
/// copy leaves out, counts as different.
fn same_tree(a: &Path, b: &Path) -> bool {
    match (library::read_tree(a), library::read_tree(b)) {
        (Ok(a), Ok(b)) => a
            .exact_digest()
            .is_some_and(|d| Some(d) == b.exact_digest()),
        _ => false,
    }
}

/// What a copy of `folder` would leave out: links, `.git` and anything that isn't a plain
/// file or folder. Only names are read, never contents; bounded like a copy.
fn left_out(folder: &Path) -> Vec<String> {
    fn walk(dir: &Path, rel: &Path, depth: usize, seen: &mut usize, out: &mut Vec<String>) {
        if depth > library::MAX_DEPTH {
            return;
        }
        for (name, path) in list_dir(dir) {
            *seen += 1;
            if *seen > library::MAX_FILES {
                return;
            }
            let here = rel.join(&name);
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() && name != ".git" => walk(&path, &here, depth + 1, seen, out),
                Ok(m) if m.is_file() => {}
                _ => out.push(here.display().to_string()),
            }
        }
    }
    let mut out = Vec::new();
    walk(folder, Path::new(""), 0, &mut 0, &mut out);
    out
}

/// Looks at everything outside Atlas. `harnesses` is the harness scan of the same moment
/// (its hooks are classified here, not read again).
#[must_use]
pub fn scan(
    sources: &Sources,
    catalog: &Catalog,
    harnesses: &[HarnessView],
    rules: &Rules,
) -> Outside {
    let placements = &catalog.placements;
    let ctx = Ctx {
        sources,
        rules,
        home: fs::canonicalize(&sources.home).unwrap_or_else(|_| sources.home.clone()),
        keep: placements.keep.clone(),
    };
    let mut out = Outside::default();
    skills(&ctx, catalog, &mut out);
    built_in(&ctx, &mut out);
    hooks(&ctx, catalog, harnesses, &mut out);
    memory(&ctx, &mut out);
    projects(&ctx, catalog, harnesses, placements, &mut out);
    out
}

fn list_dir(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut out: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .take(MAX_ENTRIES)
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    out.sort();
    out
}

fn is_md(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("md"))
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.is_dir())
}

fn has_skill_md(folder: &Path) -> bool {
    fs::symlink_metadata(folder.join("SKILL.md")).is_ok_and(|m| m.is_file())
}

fn store_homes(sources: &Sources) -> Vec<PathBuf> {
    let mut homes = vec![sources.knowledge.join("skills")];
    homes.extend(sources.public_skills.clone());
    homes.extend(
        sources
            .projects
            .iter()
            .map(|p| p.root.join(".agents/skills")),
    );
    homes
        .into_iter()
        .filter_map(|p| fs::canonicalize(p).ok())
        .collect()
}

fn skills(ctx: &Ctx, catalog: &Catalog, out: &mut Outside) {
    let store = store_homes(ctx.sources);
    let projects: Vec<(Slug, PathBuf)> = ctx
        .sources
        .projects
        .iter()
        .filter_map(|p| Some((p.name.clone(), fs::canonicalize(&p.root).ok()?)))
        .collect();
    // Real folder (or the entry, for a broken link) → the skill.
    let mut found: BTreeMap<PathBuf, OutsideSkill> = BTreeMap::new();
    let mut seen: BTreeSet<PathBuf> = BTreeSet::new();
    for layout in atlas_harness::layouts() {
        let present = ctx.sources.home.join(layout.marker).exists();
        for d in layout.skill_dirs {
            let dir = ctx.sources.home.join(d);
            for (name, path) in list_dir(&dir) {
                if name.starts_with('.') {
                    continue;
                }
                let Some((key, origin, real)) = classify_entry(ctx, &store, &name, &path, out)
                else {
                    continue;
                };
                let shown = ctx.shown(&path);
                let first = seen.insert(path.clone());
                let skill = found.entry(key).or_insert_with(|| {
                    let project = real.as_ref().and_then(|r| {
                        projects
                            .iter()
                            .find(|(_, root)| r.starts_with(root))
                            .map(|(n, _)| n.clone())
                    });
                    OutsideSkill {
                        key: Keep::key(KeepKind::Skill, &shown),
                        left_out: real.as_deref().map_or_else(Vec::new, left_out),
                        has_skill_md: real.as_deref().is_some_and(has_skill_md),
                        shown_real: real.as_deref().map_or_else(String::new, |r| ctx.shown(r)),
                        name: name.clone(),
                        entries: Vec::new(),
                        kinds: Vec::new(),
                        origin,
                        real,
                        project,
                        clash: None,
                        kept: false,
                    }
                });
                if first && !skill.entries.contains(&shown) {
                    skill.entries.push(shown);
                }
                if present && !skill.kinds.contains(&layout.kind) {
                    skill.kinds.push(layout.kind);
                }
            }
        }
    }
    for mut skill in found.into_values() {
        skill.kept = skill.entries.iter().any(|e| ctx.kept(KeepKind::Skill, e));
        skill.clash = clash(ctx, catalog, &skill);
        out.skills.push(skill);
    }
    out.skills
        .sort_by(|a, b| (&a.name, &a.key).cmp(&(&b.name, &b.key)));
}

/// What one entry of an agent's skills folder is: the grouping key (its real folder, or
/// the entry itself for a broken link), its origin and its real folder when Atlas may copy
/// it. None for Atlas's own links and for things that aren't skill folders; built-in and
/// plugin skills go to `out.built_in` instead.
fn classify_entry(
    ctx: &Ctx,
    store: &[PathBuf],
    name: &str,
    path: &Path,
    out: &mut Outside,
) -> Option<(PathBuf, Origin, Option<PathBuf>)> {
    let meta = fs::symlink_metadata(path).ok()?;
    let link = meta.file_type().is_symlink();
    if !link && !meta.is_dir() {
        return None;
    }
    let target = if link {
        fs::read_link(path).map_or_else(|_| "?".to_owned(), |t| ctx.sources.display(&t))
    } else {
        String::new()
    };
    let Ok(real) = fs::canonicalize(path) else {
        return link.then(|| (path.to_path_buf(), Origin::Broken { target }, None));
    };
    if !real.is_dir() || real.parent().is_some_and(|p| store.iter().any(|s| s == p)) {
        return None;
    }
    Some(match place_of(&ctx.home, ctx.rules, &real) {
        Place::Owner if link => (real.clone(), Origin::Linked { target }, Some(real)),
        Place::Owner => (real.clone(), Origin::OwnFolder, Some(real)),
        Place::Tool(tool) => {
            let target = if link { target } else { ctx.shown(&real) };
            (real, Origin::Tool { tool, target }, None)
        }
        Place::ClaudePlugin | Place::CodexSystem => {
            built_in_entry(ctx, name, &real, out);
            return None;
        }
    })
}

/// The Atlas skill an outside one would land on: a Global one first, else one in the
/// project it was found in. Folders are compared only here, for clashes.
fn clash(ctx: &Ctx, catalog: &Catalog, skill: &OutsideSkill) -> Option<Clash> {
    let real = skill.real.as_ref()?;
    let hit = catalog
        .skills
        .iter()
        .filter(|s| s.name.as_str() == skill.name)
        .find(|s| {
            s.home.scope() == Scope::Global
                || skill
                    .project
                    .as_ref()
                    .is_some_and(|p| s.home == SkillHome::Project(p.clone()))
        })?;
    let folder = skill_folder(ctx.sources, &hit.home, &skill.name)?;
    Some(Clash {
        home: hit.home.clone(),
        same: same_tree(real, &folder),
    })
}

fn built_in_entry(ctx: &Ctx, name: &str, real: &Path, out: &mut Outside) {
    let (agent, source) = match place_of(&ctx.home, ctx.rules, real) {
        Place::CodexSystem => (HarnessKind::Codex, "Codex's own".to_owned()),
        _ => (
            HarnessKind::ClaudeCode,
            "From a Claude Code plugin".to_owned(),
        ),
    };
    let item = BuiltIn {
        agent,
        name: name.to_owned(),
        source,
        path: ctx.shown(real),
    };
    if !out.built_in.contains(&item) {
        out.built_in.push(item);
    }
}

/// Codex's system skills and the skills of installed Claude Code plugins, as T3 Code's
/// Skills page lists them (`ProvidedSkills`): `~/.codex/skills/.system/<name>`, and each
/// plugin install's `skills/<name>` from `~/.claude/plugins/installed_plugins.json`.
fn built_in(ctx: &Ctx, out: &mut Outside) {
    let home = &ctx.sources.home;
    for (name, path) in list_dir(&home.join(".codex/skills/.system")) {
        if !name.starts_with('.') && is_real_dir(&path) && has_skill_md(&path) {
            out.built_in.push(BuiltIn {
                agent: HarnessKind::Codex,
                name,
                source: "Codex's own".to_owned(),
                path: ctx.shown(&path),
            });
        }
    }
    let file = home.join(".claude/plugins/installed_plugins.json");
    let small = fs::symlink_metadata(&file).is_ok_and(|m| m.is_file() && m.len() <= MAX_JSON_BYTES);
    let json: Option<Value> = small
        .then(|| fs::read(&file).ok())
        .flatten()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let plugins = json
        .as_ref()
        .and_then(|j| j.get("plugins"))
        .and_then(Value::as_object);
    let projects: Vec<(Slug, PathBuf)> = ctx
        .sources
        .projects
        .iter()
        .filter_map(|p| Some((p.name.clone(), fs::canonicalize(&p.root).ok()?)))
        .collect();
    let mut dirs = BTreeSet::new();
    for (id, entry) in plugins.into_iter().flatten() {
        let plugin = id.rsplit_once('@').map_or(id.as_str(), |(n, _)| n);
        let installs = entry
            .as_array()
            .map_or_else(|| vec![entry], |a| a.iter().collect());
        for install in installs {
            let Some(path) = install
                .get("installPath")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
            else {
                continue;
            };
            let scope = install.get("scope").and_then(Value::as_str);
            let project = if matches!(scope, Some("project" | "local")) {
                let at = install
                    .get("projectPath")
                    .and_then(Value::as_str)
                    .and_then(|p| fs::canonicalize(p).ok());
                let Some(found) = projects.iter().find(|(_, r)| Some(r) == at.as_ref()) else {
                    continue;
                };
                Some(found.0.clone())
            } else {
                None
            };
            let skills = path.join("skills");
            if !dirs.insert(skills.clone()) {
                continue;
            }
            for (name, folder) in list_dir(&skills) {
                if name.starts_with('.') || !folder.is_dir() || !folder.join("SKILL.md").is_file() {
                    continue;
                }
                let item = BuiltIn {
                    agent: HarnessKind::ClaudeCode,
                    name: format!("{plugin}:{name}"),
                    source: match &project {
                        Some(p) => format!("Claude Code plugin {plugin}, in {p}"),
                        None => format!("Claude Code plugin {plugin}"),
                    },
                    path: ctx.shown(&folder),
                };
                if !out.built_in.contains(&item) {
                    out.built_in.push(item);
                }
            }
        }
    }
    out.built_in
        .sort_by(|a, b| (a.agent, &a.name).cmp(&(b.agent, &b.name)));
}

// ---------------------------------------------------------------- hooks

/// Atlas's event for a Claude Code or Codex event name.
#[must_use]
pub fn atlas_event(harness_event: &str) -> Option<HookEvent> {
    Some(match harness_event {
        "SessionStart" => HookEvent::SessionStart,
        "UserPromptSubmit" => HookEvent::UserPrompt,
        "PreToolUse" => HookEvent::PreToolUse,
        "PostToolUse" => HookEvent::PostToolUse,
        "Stop" => HookEvent::Stop,
        _ => return None,
    })
}

/// `hook.toml`'s name for an event.
#[must_use]
pub const fn event_id(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "session-start",
        HookEvent::UserPrompt => "user-prompt",
        HookEvent::PreToolUse => "pre-tool-use",
        HookEvent::PostToolUse => "post-tool-use",
        HookEvent::Stop => "stop",
    }
}

/// The words of a command, quotes removed.
fn words(command: &str) -> impl Iterator<Item = &str> {
    command
        .split_whitespace()
        .map(|w| w.trim_matches(['"', '\'']))
}

/// A path a command word names: absolute, `~/...` or `$HOME/...`.
fn word_path(home: &Path, word: &str) -> Option<PathBuf> {
    if word.starts_with('/') {
        return Some(PathBuf::from(word));
    }
    ["~/", "$HOME/", "${HOME}/"]
        .iter()
        .find_map(|p| word.strip_prefix(p))
        .map(|rest| home.join(rest))
}

/// Who put a hook command in a harness's config: Atlas (its `# atlas:` marker), a tool
/// (Herdr's `herdr-agent-*` scripts, or a script in a tool's folder), or the owner.
#[must_use]
pub fn hook_owner(home: &Path, rules: &Rules, command: &str) -> HookOwner {
    if let Some(name) = atlas_harness::hooks::managed_name(command) {
        return HookOwner::Atlas(name.to_owned());
    }
    for word in words(command) {
        let base = word.rsplit('/').next().unwrap_or(word);
        if base == "herdr" || base.starts_with("herdr-") {
            return HookOwner::Tool("Herdr".to_owned());
        }
        let Some(path) = word_path(home, word) else {
            continue;
        };
        match place_of(home, rules, &path) {
            Place::Owner => {}
            Place::Tool(Some(t)) => return HookOwner::Tool(t),
            Place::Tool(None) => return HookOwner::Tool("a tool".to_owned()),
            Place::ClaudePlugin => return HookOwner::Tool("a Claude Code plugin".to_owned()),
            Place::CodexSystem => return HookOwner::Tool("Codex".to_owned()),
        }
    }
    HookOwner::Owner
}

/// Lowercase-with-hyphens from any text, or None if nothing is left.
fn slugify(text: &str) -> Option<String> {
    let mut s = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c.to_ascii_lowercase());
        } else if !s.ends_with('-') && !s.is_empty() {
            s.push('-');
        }
    }
    let s = s.trim_end_matches('-').to_owned();
    Slug::new(s.as_str()).is_ok().then_some(s)
}

/// A name for an imported hook: its script's file name, else agent and event.
#[must_use]
pub fn suggest_hook_name(kind: HarnessKind, event: &str, command: &str) -> String {
    words(command)
        .filter(|w| w.contains('/') || w.contains('.'))
        .find_map(|w| {
            let base = w.rsplit('/').next()?;
            let stem = base.split_once('.').map_or(base, |(s, _)| s);
            slugify(stem)
        })
        .or_else(|| slugify(&format!("{}-{event}", kind.id())))
        .unwrap_or_else(|| "hook".to_owned())
}

fn fingerprint(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
    }
    h.finalize()[..6].iter().fold(String::new(), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn hooks(ctx: &Ctx, catalog: &Catalog, harnesses: &[HarnessView], out: &mut Outside) {
    let mut taken: BTreeSet<String> = catalog
        .hooks
        .iter()
        .filter(|h| h.scope == Scope::Global)
        .map(|h| h.name.to_string())
        .collect();
    for h in harnesses {
        let config = h.hook_config.clone().unwrap_or_default();
        for f in &h.hooks {
            let owner = hook_owner(&ctx.sources.home, ctx.rules, &f.command);
            if matches!(owner, HookOwner::Atlas(_)) || f.command.trim().is_empty() {
                continue;
            }
            let timeout = f.timeout.map_or_else(String::new, |t| t.to_string());
            let shown = format!(
                "{config}#{}",
                fingerprint(&[
                    h.kind.id(),
                    &f.event,
                    f.matcher.as_deref().unwrap_or(""),
                    &f.command,
                    &timeout,
                ])
            );
            let base = suggest_hook_name(h.kind, &f.event, &f.command);
            let mut name = base.clone();
            let mut n = 2;
            while taken.contains(&name) {
                name = format!("{base}-{n}");
                n += 1;
            }
            taken.insert(name.clone());
            out.hooks.push(OutsideHook {
                kept: ctx.kept(KeepKind::Hook, &shown),
                key: Keep::key(KeepKind::Hook, &shown),
                kind: h.kind,
                config: config.clone(),
                event: f.event.clone(),
                matcher: f.matcher.clone(),
                command: f.command.clone(),
                timeout: f.timeout,
                owner,
                atlas_event: atlas_event(&f.event),
                name,
            });
        }
    }
}

// ---------------------------------------------------------------- memory

fn memory(ctx: &Ctx, out: &mut Outside) {
    let home = &ctx.sources.home;
    let mut registered = BTreeSet::new();
    for p in &ctx.sources.projects {
        let folder = atlas_harness::claude_project_memory(home, &p.root);
        if let Some(key) = folder
            .parent()
            .and_then(Path::file_name)
            .and_then(|n| n.to_str())
        {
            registered.insert(key.to_owned());
        }
        let Ok(meta) = fs::symlink_metadata(&folder) else {
            continue;
        };
        // A link is atlasd's work: the folder already is the store's.
        if meta.file_type().is_symlink() || !meta.is_dir() {
            continue;
        }
        let shown = ctx.shown(&folder);
        let store = ctx
            .sources
            .knowledge
            .join("memory/projects")
            .join(p.name.as_str());
        let mut notes = Vec::new();
        let mut problem = None;
        match library::read_tree(&folder) {
            Err(e) => problem = Some(e.to_string()),
            Ok(tree) => {
                for f in &tree.files {
                    let Some(file) = f.rel.to_str().filter(|r| !r.contains('/')) else {
                        continue;
                    };
                    if !is_md(file) {
                        continue;
                    }
                    let at = store.join(file);
                    let state = match fs::symlink_metadata(&at) {
                        Err(_) => NoteState::New,
                        Ok(m) if m.is_file() && m.len() <= MAX_NOTE_BYTES => {
                            if fs::read(&at).is_ok_and(|b| b == f.bytes) {
                                NoteState::Same
                            } else {
                                NoteState::Differs
                            }
                        }
                        Ok(_) => NoteState::Differs,
                    };
                    let item = format!("{shown}/{file}");
                    notes.push(Note {
                        kept: ctx.kept(KeepKind::Memory, &item),
                        key: Keep::key(KeepKind::Memory, &item),
                        file: file.to_owned(),
                        state,
                    });
                }
            }
        }
        if notes.is_empty() && problem.is_none() {
            continue;
        }
        out.memory.push(ProjectNotes {
            project: p.name.clone(),
            folder,
            shown,
            notes,
            problem,
        });
    }
    for (key, path) in list_dir(&home.join(".claude/projects")) {
        let folder = path.join("memory");
        if registered.contains(&key) || !is_real_dir(&folder) {
            continue;
        }
        let notes = list_dir(&folder)
            .iter()
            .filter(|(n, p)| is_md(n) && fs::symlink_metadata(p).is_ok_and(|m| m.is_file()))
            .count();
        if notes > 0 {
            out.stray_memory.push(StrayNotes { key, notes });
        }
    }
}

// ---------------------------------------------------------------- projects

fn projects(
    ctx: &Ctx,
    catalog: &Catalog,
    harnesses: &[HarnessView],
    placements: &Placements,
    out: &mut Outside,
) {
    for p in &ctx.sources.projects {
        for (name, path) in list_dir(&p.root.join(".claude/skills")) {
            if name.starts_with('.') || !is_real_dir(&path) || !has_skill_md(&path) {
                continue;
            }
            let taken = fs::symlink_metadata(p.root.join(".agents/skills").join(&name)).is_ok();
            out.claude_only.push(ClaudeOnly {
                project: p.name.clone(),
                name,
                taken,
            });
        }
    }
    for s in &catalog.skills {
        let SkillHome::Project(project) = &s.home else {
            continue;
        };
        let Some(global) = catalog
            .skills
            .iter()
            .find(|g| g.home.scope() == Scope::Global && g.name == s.name)
        else {
            continue;
        };
        let same = match (
            skill_folder(ctx.sources, &s.home, s.name.as_str()),
            skill_folder(ctx.sources, &global.home, s.name.as_str()),
        ) {
            (Some(a), Some(b)) => same_tree(&a, &b),
            _ => false,
        };
        out.duplicates.push(Duplicate {
            project: project.clone(),
            name: s.name.clone(),
            global: global.home.clone(),
            same,
        });
    }
    let present: Vec<HarnessKind> = harnesses
        .iter()
        .filter(|h| h.present)
        .map(|h| h.kind)
        .collect();
    for (project, skills) in &placements.project {
        for (name, placement) in skills {
            let exists = catalog
                .skills
                .iter()
                .any(|s| &s.name == name && s.home == SkillHome::Project(project.clone()));
            let agents: Vec<HarnessKind> = placement
                .off
                .iter()
                .copied()
                .filter(|k| present.contains(k))
                .collect();
            if exists && !agents.is_empty() {
                out.switched_off.push(SwitchedOff {
                    project: project.clone(),
                    name: name.clone(),
                    agents,
                });
            }
        }
    }
}

// ---------------------------------------------------------------- what an import writes

/// The script of an imported hook: the hand-made command, run by `sh` as the harness ran
/// it (`sh -c`), with the same input and working folder.
#[must_use]
pub fn hook_script(hook: &OutsideHook) -> String {
    format!(
        "#!/bin/sh\n# Brought into Atlas from {} ({}).\n{}\n",
        hook.config,
        hook.event,
        hook.command.trim()
    )
}

/// The script's file name inside the hook's folder.
pub const HOOK_SCRIPT: &str = "run.sh";

/// `hook.toml` for an imported hook (decision 0011): its event in Atlas's words, matcher
/// and timeout carried, limited to the agent it came from, and `replaces` naming the
/// hand-made entry so atlasd's managed one takes its place.
///
/// # Errors
/// The hook's event has no Atlas name, or the file wouldn't read back as a manifest.
pub fn hook_manifest(hook: &OutsideHook) -> Result<String, String> {
    let event = hook
        .atlas_event
        .ok_or_else(|| format!("Atlas has no {} event", hook.event))?;
    let q = |s: &str| toml::Value::String(s.to_owned()).to_string();
    let mut text = format!(
        "description = {}\nevent = \"{}\"\n",
        q(&format!(
            "Brought in from {}'s {}",
            hook.kind.display_name(),
            hook.config
        )),
        event_id(event)
    );
    if let Some(m) = &hook.matcher {
        let _ = writeln!(text, "matcher = {}", q(m));
    }
    let _ = writeln!(text, "run = \"{HOOK_SCRIPT}\"");
    if let Some(t) = hook.timeout.and_then(|t| u32::try_from(t).ok()) {
        let _ = writeln!(text, "timeout = {t}");
    }
    let _ = writeln!(
        text,
        "harnesses = [\"{}\"]\nreplaces = {}",
        hook.kind.id(),
        q(&hook.command)
    );
    toml::from_str::<HookManifest>(&text).map_err(|e| e.message().to_owned())?;
    Ok(text)
}

/// The store's `memory/projects/<project>/index.md` after bringing in `files` from
/// `host`: the notes are copied byte for byte (so atlasd can link Claude Code's folder
/// to the store's), and this map records where each came from. Lines already there stay.
#[must_use]
pub fn notes_index(existing: Option<&str>, project: &str, host: &str, files: &[String]) -> String {
    let mut text = existing.map_or_else(
        || {
            format!(
                "# {project}: Claude Code's notes\n\nClaude Code's own notes on {project}, brought into Atlas. Each line says which\nmachine a note came from.\n\n"
            )
        },
        str::to_owned,
    );
    if !text.ends_with('\n') {
        text.push('\n');
    }
    for f in files {
        if !text.contains(&format!("]({f})")) {
            let _ = writeln!(text, "- [{f}]({f}) · from {host}");
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules() -> Rules {
        Rules::default()
    }

    #[test]
    fn places_tell_the_owners_files_from_tools() {
        let home = Path::new("/home/u");
        let at = |p: &str| place_of(home, &rules(), Path::new(p));
        assert_eq!(at("/home/u/Projects/app/skills/x"), Place::Owner);
        assert_eq!(at("/home/u/.claude/skills/x"), Place::Owner);
        assert_eq!(at("/home/u/.config/opencode/skills/x"), Place::Owner);
        assert_eq!(at("/home/u/.agents/skills/x"), Place::Owner);
        assert_eq!(
            at("/home/u/.cua-driver/skills/cua"),
            Place::Tool(Some("cua-driver".into()))
        );
        assert_eq!(
            at("/home/u/.local/share/omarchy/skills/x"),
            Place::Tool(Some("omarchy".into()))
        );
        assert_eq!(
            at("/home/u/.config/herdr/skills/x"),
            Place::Tool(Some("herdr".into()))
        );
        assert_eq!(at("/home/u/.t3/skills/x"), Place::Tool(Some("t3".into())));
        assert_eq!(
            at("/home/u/code/node_modules/pkg/skills/x"),
            Place::Tool(None)
        );
        assert_eq!(
            at("/usr/share/omarchy/skills/x"),
            Place::Tool(Some("omarchy".into()))
        );
        assert_eq!(
            at("/nix/store/0123456789abcdef0123456789abcdef-tool-1.0/skills/x"),
            Place::Tool(Some("tool-1.0".into()))
        );
        assert_eq!(at("/srv/elsewhere/x"), Place::Tool(None));
        assert_eq!(
            at("/home/u/.claude/plugins/cache/m/p/1/skills/x"),
            Place::ClaudePlugin
        );
        assert_eq!(at("/home/u/.codex/skills/.system/x"), Place::CodexSystem);
    }

    #[test]
    fn hook_owners() {
        let home = Path::new("/home/u");
        let who = |c: &str| hook_owner(home, &rules(), c);
        assert_eq!(
            who("python3 ~/.claude/hooks/no-worktrees.py"),
            HookOwner::Owner
        );
        assert_eq!(who("echo done"), HookOwner::Owner);
        assert_eq!(
            who("/home/u/.claude/hooks/herdr-agent-state.sh"),
            HookOwner::Tool("Herdr".into())
        );
        assert_eq!(
            who("bash $HOME/.cua-driver/hook.sh"),
            HookOwner::Tool("cua-driver".into())
        );
        assert_eq!(
            who("/opt/atlas/hooks/guard/run.sh # atlas:guard"),
            HookOwner::Atlas("guard".into())
        );
    }

    #[test]
    fn hook_names_come_from_the_script() {
        assert_eq!(
            suggest_hook_name(
                HarnessKind::ClaudeCode,
                "PreToolUse",
                "python3 ~/.claude/hooks/No_Worktrees.py"
            ),
            "no-worktrees"
        );
        assert_eq!(
            suggest_hook_name(HarnessKind::Codex, "Stop", "echo done"),
            "codex-stop"
        );
    }

    fn found(command: &str, event: &str) -> OutsideHook {
        OutsideHook {
            key: String::new(),
            kind: HarnessKind::ClaudeCode,
            config: "~/.claude/settings.json".into(),
            event: event.into(),
            matcher: Some("Bash".into()),
            command: command.into(),
            timeout: Some(10),
            owner: HookOwner::Owner,
            atlas_event: atlas_event(event),
            name: "x".into(),
            kept: false,
        }
    }

    #[test]
    fn an_imported_hook_toml_is_a_valid_manifest() {
        let hook = found(
            "python3 \"~/.claude/hooks/a b.py\" --flag='x'",
            "PreToolUse",
        );
        let text = hook_manifest(&hook).unwrap();
        let m: HookManifest = toml::from_str(&text).unwrap();
        assert_eq!(m.event, HookEvent::PreToolUse);
        assert_eq!(m.matcher.as_deref(), Some("Bash"));
        assert_eq!(m.run, HOOK_SCRIPT);
        assert_eq!(m.timeout, Some(10));
        assert_eq!(m.harnesses, Some(vec![HarnessKind::ClaudeCode]));
        assert_eq!(m.replaces.as_deref(), Some(hook.command.as_str()));
        assert!(hook_script(&hook).starts_with("#!/bin/sh\n"));
        assert!(hook_script(&hook).ends_with("--flag='x'\n"));
        assert!(hook_manifest(&found("x", "Notification")).is_err());
        assert!(found("x", "Notification").blocked().is_some());
        assert!(found("x", "Stop").blocked().is_none());
    }

    #[test]
    fn the_notes_index_keeps_what_is_there() {
        let first = notes_index(None, "shop", "box", &["a.md".into()]);
        assert!(first.contains("- [a.md](a.md) · from box\n"));
        let again = notes_index(
            Some(&first),
            "shop",
            "other",
            &["a.md".into(), "b.md".into()],
        );
        assert!(again.starts_with(&first));
        assert!(again.ends_with("- [b.md](b.md) · from other\n"));
        assert_eq!(again.matches("](a.md)").count(), 1);
    }
}
