//! The Distribution context. Phase 1 only observes: what each harness on a machine
//! currently sees, and whether that came from Atlas.

use crate::catalog::SkillHome;
use crate::kernel::{HarnessKind, Slug};

/// What one entry in a harness's skills folder is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstalledSkill {
    /// A link to a skill in the store.
    FromAtlas { home: SkillHome, name: Slug },
    /// A real folder, or a link somewhere Atlas doesn't manage.
    Outside { name: String, detail: String },
    /// A link whose target is gone.
    Broken { name: String, target: String },
}

impl InstalledSkill {
    #[must_use]
    pub fn name(&self) -> &str {
        match self {
            Self::FromAtlas { name, .. } => name.as_str(),
            Self::Outside { name, .. } | Self::Broken { name, .. } => name,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillDir {
    /// Display path (home shown as `~`).
    pub path: String,
    pub exists: bool,
    pub entries: Vec<InstalledSkill>,
}

/// How a harness's instruction file relates to the global instructions in the store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionLink {
    Missing,
    /// A symlink to the store's file.
    Linked,
    /// Its own file that imports the store's file (`@path`).
    Imports,
    /// Its own file, unrelated to the store.
    Separate,
}

#[derive(Clone, Debug)]
pub struct InstructionView {
    pub path: String,
    pub link: InstructionLink,
}

/// A hook found in a harness's own config, not defined in Atlas.
#[derive(Clone, Debug)]
pub struct FoundHook {
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
    pub timeout: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct HarnessView {
    pub kind: HarnessKind,
    /// Whether the harness looks installed for this login.
    pub present: bool,
    pub skill_dirs: Vec<SkillDir>,
    pub instructions: Vec<InstructionView>,
    pub hook_config: Option<String>,
    pub hooks: Vec<FoundHook>,
}

impl HarnessView {
    /// Every skill name this harness can see, across its folders.
    pub fn skill_names(&self) -> impl Iterator<Item = &str> {
        self.skill_dirs
            .iter()
            .flat_map(|d| d.entries.iter().map(InstalledSkill::name))
    }

    #[must_use]
    pub fn sees(&self, name: &str) -> bool {
        self.skill_names().any(|n| n == name)
    }
}

/// What one harness reads inside one project repo on this machine.
#[derive(Clone, Debug)]
pub struct ProjectHarness {
    pub kind: HarnessKind,
    /// Skill names it can load from the repo's skill folders.
    pub skills: Vec<String>,
    /// The instruction file it reads in the repo root, if any.
    pub instructions: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProjectView {
    pub project: Slug,
    pub harnesses: Vec<ProjectHarness>,
}

// ---------------------------------------------------------------- hub ↔ client messages

/// A client asks to pair, with the one-time code the owner got from the hub.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PairRequest {
    pub code: String,
    /// The hub address the client was given, so a mismatch is refused before the code
    /// is used up.
    #[serde(default)]
    pub hub: String,
    /// The machine's name, such as its hostname.
    pub machine: String,
    /// The OS account the client runs as.
    pub user: String,
}

/// The hub's answer: the client's own token, and the owner keys it should trust for
/// release signatures from now on (until a signed release changes them).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PairResponse {
    pub client_id: String,
    pub token: String,
    pub hub: String,
    pub owner_keys: Vec<crate::release::OwnerKey>,
}

/// How one harness looks on a client after it applied a release.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HarnessReport {
    pub kind: HarnessKind,
    pub present: bool,
    pub skills_from_atlas: u32,
    pub skills_outside: u32,
    pub broken: u32,
    /// `linked`, `imports`, `separate` or `missing`.
    pub instructions: String,
    pub hooks_managed: u32,
    /// Subagent definitions linked from Atlas's copies (Claude Code only so far). Absent
    /// in reports from older clients.
    #[serde(default)]
    pub agents_from_atlas: u32,
}

/// What one outside skill entry is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutsideKind {
    /// A real folder of its own.
    Folder,
    /// A link to a folder Atlas doesn't manage.
    Link,
    /// A link whose target is gone.
    Broken,
}

/// A skill in an agent's folder on a machine that isn't Atlas's, so the hub can show
/// what each machine has outside Atlas (decision 0027).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct OutsideSkill {
    pub name: String,
    /// The entry's path, home shown as `~`.
    pub path: String,
    pub kind: OutsideKind,
    /// For a link: where it points, as written in it (home shown as `~`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// `Tree::exact_digest` of the folder (through a link too), when it holds only plain
    /// files and folders within the limits: equal digests are the same skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// What a client tells the hub after each run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Report {
    /// The release now applied, if any.
    pub release: Option<u64>,
    /// Seconds since the Unix epoch.
    pub at: u64,
    pub harnesses: Vec<HarnessReport>,
    /// Things the client couldn't do, in words.
    pub problems: Vec<String>,
    /// Skills seen in the agents' home folders that aren't Atlas's, at most a few hundred.
    /// Absent in reports from older clients.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outside: Vec<OutsideSkill>,
}
