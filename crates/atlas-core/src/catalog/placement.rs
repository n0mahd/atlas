//! Where each skill goes: which agents get it and, for a global skill, whether it is used
//! everywhere or only in some projects. Stored in the knowledge repo as
//! `atlas/placements.toml`, which lists only exceptions: a skill that isn't listed is on for
//! every agent, everywhere its home implies (decision 0027).
//!
//! ```toml
//! [global.pdf-tools]
//! off = ["codex", "pi"]           # these agents don't get it
//! only-projects = ["atlas"]       # used in these projects only, not globally
//!
//! [project.atlas.release-notes]  # a skill in atlas's own .agents/skills
//! off = ["claude-code"]
//!
//! [keep]                          # outside items the owner chose to leave (Tidy up)
//! items = ["skill:~/.claude/skills/omarchy"]
//! ```

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::SkillHome;
use crate::kernel::{HarnessKind, Slug};

/// The file's path inside the knowledge repo.
pub const PLACEMENTS_FILE: &str = "atlas/placements.toml";

/// One skill's exceptions. The default (nothing set) means on for every agent, everywhere.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Placement {
    /// Agents that must not get the skill.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub off: BTreeSet<HarnessKind>,
    /// For a global skill: use it only inside these projects, not in the login's home.
    /// `None` means globally. Ignored for a project's own skill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub only_projects: Option<BTreeSet<Slug>>,
}

impl Placement {
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.off.is_empty() && self.only_projects.is_none()
    }

    /// Whether `kind` should get the skill.
    #[must_use]
    pub fn wants(&self, kind: HarnessKind) -> bool {
        !self.off.contains(&kind)
    }

    /// Whether a global skill is used in the login's home (not only in some projects).
    #[must_use]
    pub const fn global(&self) -> bool {
        self.only_projects.is_none()
    }

    /// Whether a global skill is used inside `project`. A global skill reaches every
    /// project through the home folders already, so this is only true when it is
    /// limited to some projects and `project` is one of them.
    #[must_use]
    pub fn linked_into(&self, project: &Slug) -> bool {
        self.only_projects
            .as_ref()
            .is_some_and(|p| p.contains(project))
    }
}

/// Outside items the owner chose to leave where they are, so Tidy up stops offering them.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Keep {
    /// `<kind>:<path as shown>`, such as `skill:~/.claude/skills/omarchy`.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub items: BTreeSet<String>,
}

/// The whole `atlas/placements.toml`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Placements {
    /// Global skills (private and public homes share one namespace), by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub global: BTreeMap<Slug, Placement>,
    /// A project's own skills, by project then skill name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub project: BTreeMap<Slug, BTreeMap<Slug, Placement>>,
    #[serde(default, skip_serializing_if = "keep_is_empty")]
    pub keep: Keep,
}

fn keep_is_empty(k: &Keep) -> bool {
    k.items.is_empty()
}

static DEFAULT: Placement = Placement {
    off: BTreeSet::new(),
    only_projects: None,
};

impl Placements {
    /// The placement of a skill; the default when it isn't listed.
    #[must_use]
    pub fn of(&self, home: &SkillHome, name: &Slug) -> &Placement {
        let found = match home {
            SkillHome::Private | SkillHome::Public => self.global.get(name),
            SkillHome::Project(p) => self.project.get(p).and_then(|m| m.get(name)),
        };
        found.unwrap_or(&DEFAULT)
    }

    /// Sets a skill's placement, dropping the entry when it is back to the default so the
    /// file keeps listing only exceptions.
    pub fn set(&mut self, home: &SkillHome, name: &Slug, placement: Placement) {
        let mut placement = placement;
        if matches!(home, SkillHome::Project(_)) {
            placement.only_projects = None;
        }
        match home {
            SkillHome::Private | SkillHome::Public => {
                if placement.is_default() {
                    self.global.remove(name);
                } else {
                    self.global.insert(name.clone(), placement);
                }
            }
            SkillHome::Project(p) => {
                let map = self.project.entry(p.clone()).or_default();
                if placement.is_default() {
                    map.remove(name);
                } else {
                    map.insert(name.clone(), placement);
                }
                if map.is_empty() {
                    self.project.remove(p);
                }
            }
        }
    }

    /// Forgets a skill (after it is deleted or moved to another home).
    pub fn remove(&mut self, home: &SkillHome, name: &Slug) {
        self.set(home, name, Placement::default());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slug(s: &str) -> Slug {
        Slug::new(s).unwrap()
    }

    #[test]
    fn unlisted_skills_are_on_everywhere() {
        let p = Placements::default();
        let got = p.of(&SkillHome::Private, &slug("x"));
        assert!(got.wants(HarnessKind::Codex) && got.global());
    }

    #[test]
    fn setting_the_default_removes_the_entry() {
        let mut p = Placements::default();
        let home = SkillHome::Project(slug("atlas"));
        let mut off = Placement::default();
        off.off.insert(HarnessKind::ClaudeCode);
        p.set(&home, &slug("x"), off);
        assert!(!p.of(&home, &slug("x")).wants(HarnessKind::ClaudeCode));
        p.set(&home, &slug("x"), Placement::default());
        assert_eq!(p, Placements::default());
    }

    #[test]
    fn a_project_skill_cannot_be_limited_to_projects() {
        let mut p = Placements::default();
        let home = SkillHome::Project(slug("atlas"));
        p.set(
            &home,
            &slug("x"),
            Placement {
                off: BTreeSet::new(),
                only_projects: Some(BTreeSet::from([slug("other")])),
            },
        );
        assert_eq!(p, Placements::default());
    }
}
