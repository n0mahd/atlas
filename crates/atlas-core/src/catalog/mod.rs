//! The Catalog context: skills, instruction files, memory and hooks as they sit in git.

pub mod edit;
mod frontmatter;
mod hook;
mod memory;
pub mod placement;
mod skill;

pub use frontmatter::{Frontmatter, FrontmatterError, split_frontmatter};
pub use hook::{HookDef, HookEvent, HookManifest};
pub use memory::{MemoryFrontmatter, MemoryKind, MemoryMetadata, MemoryPage};
pub use placement::{
    ClaudeAgentsMd, InstructionSettings, Keep, PLACEMENTS_FILE, Placement, Placements,
};
pub use skill::{Skill, SkillFrontmatter, SkillHome};

use crate::kernel::Scope;
use crate::untrusted::{Markdown, Untrusted};

/// An instruction file (`AGENTS.md`, `CLAUDE.md`) for every agent or for one project.
#[derive(Clone, Debug)]
pub struct InstructionFile {
    pub scope: Scope,
    /// The file name, such as `AGENTS.md`.
    pub file_name: String,
    /// Where it lives, for display (home shown as `~`).
    pub path: String,
    pub body: Untrusted<Markdown>,
}

/// Something in the store Atlas could not read as intended. Shown, never hidden.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    pub path: String,
    pub message: String,
}

/// Everything the hub read from git, at one moment.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub skills: Vec<Skill>,
    pub instructions: Vec<InstructionFile>,
    pub memory: Vec<MemoryPage>,
    pub hooks: Vec<HookDef>,
    pub problems: Vec<Problem>,
}

impl Catalog {
    /// Puts every list in a stable display order.
    pub fn sort(&mut self) {
        self.skills
            .sort_by(|a, b| (a.home.rank(), &a.name).cmp(&(b.home.rank(), &b.name)));
        self.instructions
            .sort_by(|a, b| (&a.scope, &a.file_name).cmp(&(&b.scope, &b.file_name)));
        self.memory.sort_by(|a, b| a.name.cmp(&b.name));
        self.hooks
            .sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
        self.problems.sort_by(|a, b| a.path.cmp(&b.path));
    }
}
