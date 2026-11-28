use serde::Deserialize;

use crate::kernel::{Scope, Slug};
use crate::untrusted::{Markdown, Untrusted};

/// Where a skill's source lives.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum SkillHome {
    /// The private knowledge repo.
    Private,
    /// The public skills repo.
    Public,
    /// A registered project's `.agents/skills`.
    Project(Slug),
}

impl SkillHome {
    #[must_use]
    pub fn scope(&self) -> Scope {
        match self {
            Self::Private | Self::Public => Scope::Global,
            Self::Project(p) => Scope::Project(p.clone()),
        }
    }

    /// The URL segment for this home: `private`, `public` or the project name.
    #[must_use]
    pub fn key(&self) -> &str {
        match self {
            Self::Private => "private",
            Self::Public => "public",
            Self::Project(p) => p.as_str(),
        }
    }

    pub(crate) const fn rank(&self) -> u8 {
        match self {
            Self::Public => 0,
            Self::Private => 1,
            Self::Project(_) => 2,
        }
    }
}

/// A skill folder with a `SKILL.md`.
#[derive(Clone, Debug)]
pub struct Skill {
    /// The folder name. Harnesses link skills by folder name, so this is the identity.
    pub name: Slug,
    pub home: SkillHome,
    pub description: String,
    /// The `name` field in the frontmatter when it differs from the folder name.
    pub declared_name: Option<String>,
    /// Where `SKILL.md` lives, for display (home shown as `~`).
    pub path: String,
    /// Other files in the skill folder, relative to it.
    pub files: Vec<String>,
    /// Why machines won't install it, in atlasd's words, when the folder breaks the rule
    /// for what Atlas installs (a link leading out of the skill's own folder).
    pub skipped: Option<String>,
    pub body: Untrusted<Markdown>,
}

/// The frontmatter fields Atlas reads from `SKILL.md`. Others are kept in the file but
/// ignored here.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct SkillFrontmatter {
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
}
