//! Which folders each skill's links go in, and which agents must switch which skills
//! off, from `atlas_harness::placement::solve` (decision 0027). Skills with the same
//! agents off share one solution, so hundreds of skills cost a handful of solves.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use atlas_core::kernel::HarnessKind;
use atlas_harness::Layout;
use atlas_harness::placement::{Reach, Solved, solve};
use atlas_harness::switches::can_switch_off;

/// The folders a set of skills' links go in, and the agents that must switch some off.
#[derive(Clone, Debug, Default)]
pub(crate) struct Placed {
    /// Folders (relative to the home or the repo) and what each should hold, name →
    /// source, in the order they are planned.
    pub folders: Vec<(&'static str, BTreeMap<String, PathBuf>)>,
    /// Agent → skill name → the folder it loads that skill from, for its switch.
    pub off: BTreeMap<HarnessKind, BTreeMap<String, PathBuf>>,
    /// Skills an agent will see although they are off for it, in words.
    pub stuck: Vec<String>,
}

pub(crate) struct Placer<'a> {
    harnesses: &'a [&'static Layout],
    reach: Reach,
    /// The home or the repo: folders are relative to it.
    base: &'a Path,
    cache: BTreeMap<(BTreeSet<HarnessKind>, Vec<&'static str>), Solved>,
    pub placed: Placed,
}

impl<'a> Placer<'a> {
    pub(crate) fn new(harnesses: &'a [&'static Layout], reach: Reach, base: &'a Path) -> Self {
        Self {
            harnesses,
            reach,
            base,
            cache: BTreeMap::new(),
            placed: Placed::default(),
        }
    }

    fn solved(&mut self, off: &BTreeSet<HarnessKind>, own: &[&'static str]) -> Solved {
        let (harnesses, reach) = (self.harnesses, self.reach);
        self.cache
            .entry((off.clone(), own.to_vec()))
            .or_insert_with(|| {
                let wanted = HarnessKind::ALL
                    .into_iter()
                    .filter(|k| !off.contains(k))
                    .collect();
                solve(harnesses, &wanted, reach, own, |k| can_switch_off(k, reach))
            })
            .clone()
    }

    fn folder(&mut self, folder: &'static str) -> &mut BTreeMap<String, PathBuf> {
        let at = self
            .placed
            .folders
            .iter()
            .position(|(f, _)| *f == folder)
            .unwrap_or_else(|| {
                self.placed.folders.push((folder, BTreeMap::new()));
                self.placed.folders.len() - 1
            });
        &mut self.placed.folders[at].1
    }

    /// Lists the folders used when nothing is off, so a skill that leaves one is unlinked
    /// there. With every agent wanted that is each harness's first folder, unless a folder
    /// it reads already has the links (`OpenCode` reads Claude's and Codex's), and never a
    /// harness's other folders, or it would see every skill twice (Codex reads both
    /// `~/.agents/skills` and `~/.codex/skills`).
    pub(crate) fn seed(&mut self, own: &[&'static str]) {
        for f in self.solved(&BTreeSet::new(), own).folders {
            self.folder(f);
        }
    }

    /// Places one skill whose links point at `source`. `own` lists folders that already
    /// hold the skill itself.
    pub(crate) fn add(
        &mut self,
        name: &str,
        source: &Path,
        off: &BTreeSet<HarnessKind>,
        own: &[&'static str],
    ) {
        let s = self.solved(off, own);
        for f in &s.folders {
            self.folder(f).insert(name.to_owned(), source.to_path_buf());
        }
        for &kind in &s.switch_off {
            let reads = self
                .harnesses
                .iter()
                .filter(|l| l.kind == kind)
                .flat_map(|l| self.reach.dirs(l))
                .find(|d| s.folders.contains(d) || own.contains(d));
            if let Some(dir) = reads {
                let path = self.base.join(dir).join(name);
                self.placed
                    .off
                    .entry(kind)
                    .or_default()
                    .insert(name.to_owned(), path);
            }
        }
        if !s.stuck_on.is_empty() {
            let who: Vec<&str> = s.stuck_on.iter().map(|k| k.display_name()).collect();
            self.placed.stuck.push(format!(
                "skill {name} is off for {}, but Atlas can't keep it from them: they read every \
                 folder that reaches the agents using it and have no setting to switch it off",
                who.join(", ")
            ));
        }
    }
}
