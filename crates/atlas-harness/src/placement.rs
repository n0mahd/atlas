//! Which folders a skill's link goes in so that exactly the wanted agents see it.
//!
//! Agents share folders: `~/.agents/skills` is read by Codex, `OpenCode`, Amp, Muse and Grok
//! Build, `~/.claude/skills` by Claude Code, `OpenCode`, Amp, Muse and Grok Build. A link
//! therefore reaches every agent that reads its folder. When an unwanted agent reads the
//! only folder that reaches a wanted one, the unwanted agent is switched off by its own
//! setting if it has one (see `switches`), and otherwise reported as unable to be
//! excluded. Pure: the caller says which agents are present.

use std::collections::BTreeSet;

use atlas_core::kernel::HarnessKind;

use crate::Layout;

/// Whether a placement is for the login's home folders or for folders inside a repo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reach {
    Home,
    Project,
}

impl Reach {
    /// The folders `layout` loads skills from at this reach.
    #[must_use]
    pub const fn dirs(self, layout: &Layout) -> &'static [&'static str] {
        match self {
            Self::Home => layout.skill_dirs,
            Self::Project => layout.project_skill_dirs,
        }
    }
}

/// The outcome for one skill.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Solved {
    /// Folders (relative to the home or the repo) that get a link, in layout order.
    pub folders: Vec<&'static str>,
    /// Present agents that read a chosen folder (or the skill's own folder) but must not
    /// use the skill, and have a setting that switches it off.
    pub switch_off: BTreeSet<HarnessKind>,
    /// Present agents that will see the skill although it is off for them: every folder
    /// that reaches a wanted agent is also read by them, and they have no such setting.
    pub stuck_on: BTreeSet<HarnessKind>,
}

impl Solved {
    /// Whether `kind` will be able to use the skill.
    #[must_use]
    pub fn sees(&self, kind: HarnessKind, present: &[&Layout], reach: Reach, own: &[&str]) -> bool {
        if self.switch_off.contains(&kind) {
            return false;
        }
        present
            .iter()
            .filter(|l| l.kind == kind)
            .flat_map(|l| reach.dirs(l))
            .any(|d| self.folders.contains(d) || own.contains(d))
    }
}

/// Picks folders for one skill.
///
/// `present` are the agents set up for this login (or in this repo), in layout order.
/// `wanted` are the agents that should get the skill. `own` lists folders that already
/// hold the skill itself rather than a link, such as a project's `.agents/skills`: their
/// readers see it whatever Atlas does. `switchable` says whether an agent can switch a
/// skill off by its own setting at this reach.
///
/// With every present agent wanted and no `own` folders this picks the same folders as
/// before placements existed: each agent's first folder, unless a folder it reads already
/// has the link.
#[must_use]
pub fn solve(
    present: &[&Layout],
    wanted: &BTreeSet<HarnessKind>,
    reach: Reach,
    own: &[&str],
    switchable: impl Fn(HarnessKind) -> bool,
) -> Solved {
    let readers = |dir: &str| -> BTreeSet<HarnessKind> {
        present
            .iter()
            .filter(|l| reach.dirs(l).contains(&dir))
            .map(|l| l.kind)
            .collect()
    };
    let unwanted_readers = |dir: &str| -> BTreeSet<HarnessKind> {
        readers(dir)
            .into_iter()
            .filter(|k| !wanted.contains(k))
            .collect()
    };
    let mut out = Solved::default();
    for layout in present {
        let dirs = reach.dirs(layout);
        if !wanted.contains(&layout.kind) || dirs.is_empty() {
            continue;
        }
        let covered = dirs
            .iter()
            .any(|d| out.folders.contains(d) || own.contains(d));
        if covered {
            continue;
        }
        let clean = dirs.iter().find(|d| unwanted_readers(d).is_empty());
        let fixable = || {
            dirs.iter()
                .find(|d| unwanted_readers(d).iter().all(|&k| switchable(k)))
        };
        if let Some(dir) = clean.or_else(fixable).or_else(|| dirs.first()) {
            out.folders.push(dir);
        }
    }
    let reached: BTreeSet<HarnessKind> = out
        .folders
        .iter()
        .chain(own)
        .flat_map(|d| unwanted_readers(d))
        .collect();
    for kind in reached {
        if switchable(kind) {
            out.switch_off.insert(kind);
        } else {
            out.stuck_on.insert(kind);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layouts;

    fn present(kinds: &[HarnessKind]) -> Vec<&'static Layout> {
        layouts()
            .iter()
            .filter(|l| kinds.contains(&l.kind))
            .collect()
    }

    fn all_of(p: &[&Layout]) -> BTreeSet<HarnessKind> {
        p.iter().map(|l| l.kind).collect()
    }

    use HarnessKind::{Antigravity, ClaudeCode, Codex, Grok, OpenCode, Pi};

    #[test]
    fn everyone_wanted_matches_the_old_plan() {
        let p = present(&[ClaudeCode, Codex, OpenCode, Pi, Antigravity]);
        let got = solve(&p, &all_of(&p), Reach::Home, &[], |_| false);
        assert_eq!(
            got.folders,
            [
                ".claude/skills",
                ".agents/skills",
                ".pi/agent/skills",
                ".gemini/config/skills"
            ]
        );
        assert!(got.switch_off.is_empty() && got.stuck_on.is_empty());
    }

    #[test]
    fn a_folder_no_unwanted_agent_reads_is_preferred() {
        // Codex only: ~/.agents/skills is also OpenCode's, ~/.codex/skills isn't.
        let p = present(&[Codex, OpenCode]);
        let got = solve(&p, &BTreeSet::from([Codex]), Reach::Home, &[], |_| false);
        assert_eq!(got.folders, [".codex/skills"]);
        assert!(got.stuck_on.is_empty());
    }

    #[test]
    fn an_unwanted_reader_is_switched_off_when_it_can_be() {
        // OpenCode only: every OpenCode folder is shared with Claude Code.
        let p = present(&[ClaudeCode, OpenCode]);
        let got = solve(&p, &BTreeSet::from([OpenCode]), Reach::Home, &[], |k| {
            k == ClaudeCode
        });
        assert_eq!(got.folders, [".config/opencode/skills"]);
        assert!(got.switch_off.is_empty());
        let p = present(&[ClaudeCode, Grok]);
        let got = solve(&p, &BTreeSet::from([Grok]), Reach::Home, &[], |k| {
            k == ClaudeCode
        });
        assert_eq!(got.folders, [".grok/skills"]);
    }

    #[test]
    fn a_project_skill_reaches_its_folders_readers_whatever_atlas_does() {
        let p = present(&[ClaudeCode, Codex, Antigravity]);
        let wanted = BTreeSet::from([ClaudeCode, Antigravity]);
        let got = solve(&p, &wanted, Reach::Project, &[".agents/skills"], |k| {
            k == Codex
        });
        // Claude needs a link; Antigravity already reads .agents/skills; Codex must be
        // switched off because the skill sits in a folder it reads.
        assert_eq!(got.folders, [".claude/skills"]);
        assert_eq!(got.switch_off, BTreeSet::from([Codex]));
        assert!(got.sees(ClaudeCode, &p, Reach::Project, &[".agents/skills"]));
        assert!(!got.sees(Codex, &p, Reach::Project, &[".agents/skills"]));
    }

    #[test]
    fn an_agent_without_a_setting_is_reported_when_it_cannot_be_avoided() {
        let p = present(&[Codex, Grok]);
        // Grok reads .agents/skills too and has no off switch; Codex's own .codex/skills
        // avoids it.
        let got = solve(
            &p,
            &BTreeSet::from([Codex]),
            Reach::Project,
            &[".agents/skills"],
            |_| false,
        );
        assert_eq!(got.folders, Vec::<&str>::new());
        assert_eq!(got.stuck_on, BTreeSet::from([Grok]));
    }

    #[test]
    fn nobody_wanted_links_nothing() {
        let p = present(&[ClaudeCode, Codex]);
        let got = solve(&p, &BTreeSet::new(), Reach::Home, &[], |_| true);
        assert_eq!(got, Solved::default());
    }
}
