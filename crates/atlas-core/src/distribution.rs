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
