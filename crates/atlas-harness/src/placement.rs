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
/// The choice is the best one possible: fewest agents that see a skill they shouldn't,
/// then fewest switched off by a setting, then fewest wanted agents that see it twice,
/// then fewest links. The simple pass (each wanted agent's first folder that no unwanted
/// agent reads, else one whose unwanted readers can be switched off) is kept whenever it
/// is already best on the first two, so with every present agent wanted and no `own`
/// folders the links are exactly those atlasd made before placements existed.
#[must_use]
pub fn solve(
    present: &[&Layout],
    wanted: &BTreeSet<HarnessKind>,
    reach: Reach,
    own: &[&str],
    switchable: impl Fn(HarnessKind) -> bool,
) -> Solved {
    let simple = simple_pass(present, wanted, reach, own, &switchable);
    if simple.stuck_on.is_empty() && simple.switch_off.is_empty() {
        return simple;
    }
    let best = search(present, wanted, reach, own, &switchable);
    match best {
        Some(best)
            if (best.stuck_on.len(), best.switch_off.len())
                < (simple.stuck_on.len(), simple.switch_off.len()) =>
        {
            best
        }
        _ => simple,
    }
}

/// The agents (present, at this reach) that read `dir`.
fn readers(present: &[&Layout], reach: Reach, dir: &str) -> BTreeSet<HarnessKind> {
    present
        .iter()
        .filter(|l| reach.dirs(l).contains(&dir))
        .map(|l| l.kind)
        .collect()
}

/// Fills in `switch_off` and `stuck_on` for a set of folders.
fn finish(
    folders: Vec<&'static str>,
    present: &[&Layout],
    wanted: &BTreeSet<HarnessKind>,
    reach: Reach,
    own: &[&str],
    switchable: &impl Fn(HarnessKind) -> bool,
) -> Solved {
    let mut out = Solved {
        folders,
        ..Solved::default()
    };
    let reached: BTreeSet<HarnessKind> = out
        .folders
        .iter()
        .copied()
        .chain(own.iter().copied())
        .flat_map(|d| readers(present, reach, d))
        .filter(|k| !wanted.contains(k))
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

/// One bit per agent kind, for the search's bitmasks.
const fn bit(kind: HarnessKind) -> u16 {
    1 << kind as u16
}

/// Tries every set of folders (there are at most a dozen distinct ones) and returns the
/// best that reaches every wanted agent, or `None` when there are too many to try.
///
/// Readers are bitmasks over agent kinds, worked out once per folder, so trying a set
/// allocates nothing; a [`Solved`] is built only for a set that beats the best so far.
fn search(
    present: &[&Layout],
    wanted: &BTreeSet<HarnessKind>,
    reach: Reach,
    own: &[&str],
    switchable: &impl Fn(HarnessKind) -> bool,
) -> Option<Solved> {
    let mut dirs: Vec<&'static str> = Vec::new();
    for l in present {
        for d in reach.dirs(l) {
            if !dirs.contains(d) && !own.contains(d) {
                dirs.push(d);
            }
        }
    }
    if dirs.len() > 16 {
        return None;
    }
    let readers_of = |d: &str| {
        present
            .iter()
            .filter(|l| reach.dirs(l).contains(&d))
            .fold(0_u16, |m, l| m | bit(l.kind))
    };
    let dir_readers: Vec<u16> = dirs.iter().map(|d| readers_of(d)).collect();
    let own_readers = own.iter().fold(0_u16, |m, d| m | readers_of(d));
    let unwanted = !wanted.iter().fold(0_u16, |m, k| m | bit(*k));
    let switch = present
        .iter()
        .filter(|l| switchable(l.kind))
        .fold(0_u16, |m, l| m | bit(l.kind));
    // Per wanted agent with folders: which of `dirs` it reads, and how many `own` ones.
    let needs: Vec<(u32, u32)> = present
        .iter()
        .filter(|l| wanted.contains(&l.kind) && !reach.dirs(l).is_empty())
        .map(|l| {
            let mut mask = 0_u32;
            let mut owned = 0;
            for d in reach.dirs(l) {
                match dirs.iter().position(|x| x == d) {
                    Some(i) => mask |= 1 << i,
                    None => owned += u32::from(own.contains(d)),
                }
            }
            (mask, owned)
        })
        .collect();

    let mut best: Option<((u32, u32, u32, u32), u32)> = None;
    for mask in 0_u32..(1 << dirs.len()) {
        let mut twice = 0;
        let mut all = true;
        for &(reads, owned) in &needs {
            let seen = (reads & mask).count_ones() + owned;
            all &= seen > 0;
            twice += seen.saturating_sub(1);
        }
        if !all {
            continue;
        }
        let reached = dir_readers
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .fold(own_readers, |m, (_, r)| m | r)
            & unwanted;
        let score = (
            (reached & !switch).count_ones(),
            (reached & switch).count_ones(),
            twice,
            mask.count_ones(),
        );
        // Masks run in layout order, so on a tie the earlier folders win.
        if best.is_none_or(|(s, _)| score < s) {
            best = Some((score, mask));
        }
    }
    best.map(|(_, mask)| {
        let chosen = dirs
            .iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, d)| *d)
            .collect();
        finish(chosen, present, wanted, reach, own, switchable)
    })
}

fn simple_pass(
    present: &[&Layout],
    wanted: &BTreeSet<HarnessKind>,
    reach: Reach,
    own: &[&str],
    switchable: &impl Fn(HarnessKind) -> bool,
) -> Solved {
    let unwanted_readers = |dir: &str| -> BTreeSet<HarnessKind> {
        readers(present, reach, dir)
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
    finish(out.folders, present, wanted, reach, own, switchable)
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

    use HarnessKind::{Amp, Antigravity, ClaudeCode, Codex, Grok, Muse, OpenCode, Pi};

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
    fn an_agents_own_folder_avoids_a_shared_one() {
        // OpenCode only: its own folder isn't Claude Code's.
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
    fn an_unwanted_reader_is_switched_off_when_every_folder_is_shared() {
        // Amp in a project reads .agents/skills (Codex's too) and .claude/skills (Claude
        // Code's too).
        let p = present(&[ClaudeCode, Codex, Amp]);
        let got = solve(&p, &BTreeSet::from([Amp]), Reach::Project, &[], |k| {
            k == Codex
        });
        assert_eq!(got.folders, [".agents/skills"]);
        assert_eq!(got.switch_off, BTreeSet::from([Codex]));
        assert!(got.stuck_on.is_empty());
        let p = present(&[Codex, Amp]);
        let got = solve(&p, &BTreeSet::from([Amp]), Reach::Project, &[], |_| false);
        assert_eq!(got.folders, [".claude/skills"]);
        assert!(got.stuck_on.is_empty() && got.switch_off.is_empty());
    }

    #[test]
    fn the_fewest_agents_are_left_seeing_a_skill_they_should_not() {
        // From review: .agents/skills would reach Amp, Muse and Grok; .codex/skills only Muse.
        let p = present(&[Codex, Amp, Muse, Grok]);
        let got = solve(&p, &BTreeSet::from([Codex]), Reach::Home, &[], |_| false);
        assert_eq!(got.folders, [".codex/skills"]);
        assert_eq!(got.stuck_on, BTreeSet::from([Muse]));
        // .agents/skills would also need OpenCode switched off; .codex/skills doesn't.
        let p = present(&[Codex, OpenCode, Muse]);
        let got = solve(&p, &BTreeSet::from([Codex]), Reach::Home, &[], |k| {
            k == OpenCode
        });
        assert_eq!(got.folders, [".codex/skills"]);
        assert!(got.switch_off.is_empty());
        // A global skill used only in a project, for Amp alone.
        let p = present(&[Codex, Pi, Antigravity, Amp, Grok]);
        let got = solve(&p, &BTreeSet::from([Amp]), Reach::Project, &[], |_| false);
        assert_eq!(got.folders, [".claude/skills"]);
        assert_eq!(got.stuck_on, BTreeSet::from([Grok]));
    }

    /// The search as first written, building every candidate: the oracle the bitmask
    /// search must match exactly.
    fn reference_search(
        present: &[&Layout],
        wanted: &BTreeSet<HarnessKind>,
        reach: Reach,
        own: &[&str],
        switchable: &impl Fn(HarnessKind) -> bool,
    ) -> Option<Solved> {
        let mut dirs: Vec<&'static str> = Vec::new();
        for l in present {
            for d in reach.dirs(l) {
                if !dirs.contains(d) && !own.contains(d) {
                    dirs.push(d);
                }
            }
        }
        if dirs.len() > 16 {
            return None;
        }
        let needs: Vec<&Layout> = present
            .iter()
            .copied()
            .filter(|l| wanted.contains(&l.kind) && !reach.dirs(l).is_empty())
            .collect();
        let mut best: Option<((usize, usize, usize, usize), Solved)> = None;
        for mask in 0_u32..(1 << dirs.len()) {
            let chosen: Vec<&'static str> = dirs
                .iter()
                .enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, d)| *d)
                .collect();
            let mut twice = 0;
            let mut all = true;
            for l in &needs {
                let seen = reach
                    .dirs(l)
                    .iter()
                    .filter(|d| chosen.contains(d) || own.contains(d))
                    .count();
                all &= seen > 0;
                twice += seen.saturating_sub(1);
            }
            if !all {
                continue;
            }
            let n = chosen.len();
            let solved = finish(chosen, present, wanted, reach, own, switchable);
            let score = (solved.stuck_on.len(), solved.switch_off.len(), twice, n);
            // Masks run in layout order, so on a tie the earlier folders win.
            if best.as_ref().is_none_or(|(s, _)| score < *s) {
                best = Some((score, solved));
            }
        }
        best.map(|(_, s)| s)
    }

    #[test]
    fn every_wanted_agent_sees_it_and_no_better_choice_exists() {
        let agents: Vec<&Layout> = layouts()
            .iter()
            .filter(|l| !l.skill_dirs.is_empty())
            .collect();
        let switchable = |k: HarnessKind| matches!(k, ClaudeCode | Codex);
        let cases: [(Reach, &[&str]); 3] = [
            (Reach::Home, &[]),
            (Reach::Project, &[]),
            (Reach::Project, &[".agents/skills"]),
        ];
        for (reach, own) in cases {
            for pmask in 0_u32..(1 << agents.len()) {
                if pmask.count_ones() > 5 {
                    continue;
                }
                let p: Vec<&Layout> = agents
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| pmask & (1 << i) != 0)
                    .map(|(_, l)| *l)
                    .collect();
                for wmask in 0_u32..(1 << p.len()) {
                    let wanted: BTreeSet<HarnessKind> = p
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| wmask & (1 << i) != 0)
                        .map(|(_, l)| l.kind)
                        .collect();
                    let got = solve(&p, &wanted, reach, own, switchable);
                    for k in &wanted {
                        assert!(got.sees(*k, &p, reach, own), "{k:?} misses it: {got:?}");
                    }
                    let best = search(&p, &wanted, reach, own, &switchable);
                    // The oracle is slow; up to four agents keeps the test quick (it
                    // matched over every set of all eight when the search was rewritten).
                    if p.len() <= 4 {
                        assert_eq!(
                            best,
                            reference_search(&p, &wanted, reach, own, &switchable),
                            "{wanted:?} of {p:?}"
                        );
                    }
                    if let Some(best) = best {
                        assert!(
                            (got.stuck_on.len(), got.switch_off.len())
                                <= (best.stuck_on.len(), best.switch_off.len()),
                            "{wanted:?}: {got:?} is worse than {best:?}"
                        );
                    }
                }
            }
        }
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
