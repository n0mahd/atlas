//! The plan for one login: which links and files each present harness needs, in the
//! home and in the owner's project checkouts. Reading the current state happens here;
//! deciding is `atlas-harness`'s pure functions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use atlas_core::catalog::ClaudeAgentsMd;
use atlas_core::kernel::HarnessKind;
use atlas_harness::hooks::{ManagedHook, antigravity_hooks, claude_settings, codex_hooks};
use atlas_harness::instructions::{
    InstructionAction, InstructionConflict, InstructionFile, claude_agents_md_setting, contained,
    is_atlas_import, plan_claude_md, plan_instruction_link,
};
use atlas_harness::placement::Reach;
use atlas_harness::skills::{
    Entry, SkillConflict, SkillOp, SkillPlan, plan_skill_links, plan_skill_links_adopting,
};
use atlas_harness::switches::{Base, render_switches, switch_file};
use atlas_harness::{Change, FormatError, Layout, layouts};

use crate::checkout::Checkout;
use crate::desired::Desired;
use crate::place::{Placed, Placer};
use crate::{display, project, tree};

const MAX_FILE: u64 = 1024 * 1024;

/// One change on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    CreateDir(PathBuf),
    CreateLink {
        path: PathBuf,
        target: PathBuf,
    },
    ReplaceLink {
        path: PathBuf,
        from: PathBuf,
        to: PathBuf,
    },
    RemoveLink {
        path: PathBuf,
        target: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        bytes: Vec<u8>,
        why: &'static str,
    },
    RemoveFile {
        path: PathBuf,
    },
    /// Replace a real folder that is an exact copy of what Atlas holds with a link to it.
    /// `digest` is what the folder held when planned; it is checked again before anything
    /// is removed.
    AdoptDir {
        path: PathBuf,
        target: PathBuf,
        digest: String,
    },
    /// An instruction file planned from `was` (`None`: there was none) gets `bytes`. When
    /// the new text drops some of `was` (Claude's `CLAUDE.md` becoming just the import),
    /// `kept_in` names the store file that must still hold every dropped line when this
    /// is applied.
    WriteInstruction {
        path: PathBuf,
        was: Option<Vec<u8>>,
        bytes: Vec<u8>,
        kept_in: Option<PathBuf>,
        /// Lines of `was` that are Atlas's own import of the global instructions (perhaps
        /// at an older path): they are replaced, so they needn't be in `kept_in`.
        imports: Vec<String>,
        why: &'static str,
    },
    /// An agent's own instruction file whose every line is in `target` becomes a link to
    /// it, if it still holds `was` and `target` still holds every line of it.
    FileToLink {
        path: PathBuf,
        was: Vec<u8>,
        target: PathBuf,
    },
}

impl Action {
    /// The change in plain words.
    #[must_use]
    pub fn describe(&self, home: &Path) -> String {
        let d = |p: &Path| display(home, p);
        match self {
            Self::CreateDir(p) => format!("create folder {}", d(p)),
            Self::CreateLink { path, target } => format!("link {} → {}", d(path), d(target)),
            Self::ReplaceLink { path, from, to } => {
                format!("repoint {} from {} to {}", d(path), d(from), d(to))
            }
            Self::RemoveLink { path, target } => {
                format!("remove link {} (it pointed at {})", d(path), d(target))
            }
            Self::WriteFile { path, why, .. } | Self::WriteInstruction { path, why, .. } => {
                format!("write {} ({why})", d(path))
            }
            Self::RemoveFile { path } => {
                format!("remove {} (Atlas's entries were all it held)", d(path))
            }
            Self::AdoptDir { path, target, .. } => format!(
                "replace folder {}, a copy of what Atlas holds, with a link to {}",
                d(path),
                d(target)
            ),
            Self::FileToLink { path, target, .. } => format!(
                "replace {} with a link to {} (all its text is there)",
                d(path),
                d(target)
            ),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Plan {
    pub actions: Vec<Action>,
    /// Things Atlas wanted to change but won't, because they aren't its own.
    pub conflicts: Vec<String>,
    /// The skills each agent is told to switch off, and where, whether or not its file
    /// changes this time.
    pub switches: Vec<SwitchIntent>,
    /// Worth saying, but not a problem.
    pub notes: Vec<String>,
}

/// What a pass plans from besides the desired state.
#[derive(Clone, Debug, Default)]
pub struct Machine {
    /// The owner's checkouts of registered projects on this machine.
    pub checkouts: Vec<Checkout>,
    /// Where Claude Code's per-repo memory folders link to.
    pub memory: MemoryStore,
}

/// The folder Claude Code's per-repo memory folders become links into.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum MemoryStore {
    /// Nothing to link.
    #[default]
    None,
    /// The owner's own checkout of the knowledge repo, which their sync commits from.
    At(PathBuf),
    /// Why no memory folder is linked.
    Unusable(String),
}

/// Skills an agent must not use although it reads a folder that holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchIntent {
    pub kind: HarnessKind,
    /// The agent's own settings file.
    pub file: PathBuf,
    /// Skill name → the folder the agent loads it from.
    pub off: BTreeMap<String, PathBuf>,
}

impl Plan {
    pub(crate) fn ensure_dir(&mut self, dir: &Path) {
        let planned = self
            .actions
            .iter()
            .any(|a| matches!(a, Action::CreateDir(d) if d == dir));
        if !planned && fs::symlink_metadata(dir).is_err() {
            self.actions.push(Action::CreateDir(dir.to_path_buf()));
        }
    }
}

/// The file Atlas writes a harness's hooks to. Grok Build reads a folder of hook files,
/// so Atlas keeps its own, `atlas.json`, beside the owner's.
#[must_use]
pub fn hook_file(kind: HarnessKind, config: &Path) -> PathBuf {
    if kind == HarnessKind::Grok {
        config.join("atlas.json")
    } else {
        config.to_path_buf()
    }
}

/// The agents set up for this login. Hosts (T3 Code, Herdr) run the others and read
/// their files.
#[must_use]
pub fn present(home: &Path) -> Vec<&'static Layout> {
    layouts()
        .iter()
        .filter(|l| !l.kind.is_host() && home.join(l.marker).exists())
        .collect()
}

/// Plans every present harness: skill and subagent links, skill switches, instruction
/// files and hook configs. Links into `managed_roots` are Atlas's; the first is its own
/// local copies, which only atlasd links into.
#[must_use]
pub fn plan(home: &Path, desired: &Desired, managed_roots: &[PathBuf]) -> Plan {
    plan_machine(home, desired, managed_roots, &Machine::default())
}

/// [`plan`], plus each project checkout on this machine: its links, its exclude block,
/// its switches and Claude Code's memory folder for it.
#[must_use]
pub fn plan_machine(
    home: &Path,
    desired: &Desired,
    managed_roots: &[PathBuf],
    machine: &Machine,
) -> Plan {
    let mut plan = Plan::default();
    let harnesses = present(home);
    let managed = managed_roots;
    let skills_on = !desired.placements_invalid;
    // Where every skill goes, home and projects, before anything is planned: Codex keeps
    // the switches of both in one file, which is rendered once.
    let home_placed = skills_on.then(|| home_skills(&harnesses, desired, home));
    let projects: Vec<Option<project::Placed>> = machine
        .checkouts
        .iter()
        .map(|co| skills_on.then(|| project::place(&harnesses, desired, co)))
        .collect();
    let mut slots = Slots::default();
    if let Some(placed) = &home_placed {
        slots.add(&mut plan, home, &harnesses, Reach::Home, None, &placed.off);
    }
    for (co, placed) in machine.checkouts.iter().zip(&projects) {
        if let Some(p) = placed {
            slots.add(
                &mut plan,
                home,
                &harnesses,
                Reach::Project,
                Some(co),
                &p.placed.off,
            );
        }
    }

    let mut digests = Digests::default();
    if let Some(placed) = home_placed {
        home_links(&mut plan, home, &harnesses, placed, managed, &mut digests);
    }
    harness_files(
        &mut plan,
        home,
        &harnesses,
        desired,
        managed,
        &mut slots,
        &mut digests,
    );
    let claude = harnesses.iter().any(|l| l.kind == HarnessKind::ClaudeCode);
    for ((co, placed), slot_files) in machine
        .checkouts
        .iter()
        .zip(&projects)
        .zip(machine.checkouts.iter().map(|co| slots.in_repo(co)))
    {
        if claude {
            project::memory(&mut plan, home, managed, co, &machine.memory);
        }
        if let Some(placed) = placed {
            project::links(
                &mut plan,
                home,
                managed,
                co,
                placed,
                &slot_files,
                &mut digests,
            );
        }
    }
    for (file, slot) in std::mem::take(&mut slots.0) {
        if let Some(repo) = &slot.repo
            && repo.tracked
        {
            if !slot.off.is_empty() {
                plan.conflicts.push(format!(
                    "{} is tracked by git, so Atlas didn't switch skills off there",
                    display(home, &file)
                ));
            }
            continue;
        }
        let step = slot.step(&mut plan, &file);
        edit_config(&mut plan, home, &file, &[step]);
    }
    plan
}

/// The home's skill links, and the removal of Atlas's links from folders no skill goes
/// in any more.
fn home_links(
    plan: &mut Plan,
    home: &Path,
    harnesses: &[&'static Layout],
    placed: Placed,
    managed: &[PathBuf],
    digests: &mut Digests,
) {
    for (dir, skills) in &placed.folders {
        link_dir(
            plan,
            home,
            &home.join(dir),
            skills,
            managed,
            digests,
            "skill",
        );
    }
    let mut seen: BTreeSet<&str> = placed.folders.iter().map(|(f, _)| *f).collect();
    for dir in harnesses.iter().flat_map(|l| l.skill_dirs) {
        if seen.insert(dir)
            && let Some(own) = managed.first()
        {
            unlink_all(plan, &home.join(dir), own);
        }
    }
    plan.conflicts.extend(placed.stuck);
}

/// Each present harness's subagent links, instruction files and hook file (with Claude
/// Code's switches, which share it).
fn harness_files(
    plan: &mut Plan,
    home: &Path,
    harnesses: &[&'static Layout],
    desired: &Desired,
    managed: &[PathBuf],
    slots: &mut Slots,
    digests: &mut Digests,
) {
    let claude = harnesses.iter().any(|l| l.kind == HarnessKind::ClaudeCode);
    let store_text = desired
        .instructions
        .as_deref()
        .map(|s| read_store_text(plan, home, s));
    for layout in harnesses {
        // Subagents: only Claude Code's format is known so far (HARNESSES.md).
        if let Some(dir) = layout.agent_dir {
            link_dir(
                plan,
                home,
                &home.join(dir),
                &desired.agents,
                managed,
                digests,
                "subagent",
            );
        }
        if let (Some(store), Some(text)) = (&desired.instructions, &store_text) {
            for file in layout.instruction_files {
                instruction(
                    plan,
                    home,
                    layout.kind,
                    &home.join(file),
                    (store, text),
                    managed,
                );
            }
        }
        if let Some(config) = layout.hook_config {
            let mut wanted = desired.hooks_for(layout.kind);
            if layout.kind == HarnessKind::Grok && claude {
                // Grok Build also runs ~/.claude/settings.json's hooks, so only hooks Claude's
                // copy doesn't carry go in its own file; the rest would run twice.
                let via_claude = desired.hooks_for(HarnessKind::ClaudeCode);
                wanted.retain(|h| !via_claude.iter().any(|c| c.name == h.name));
            }
            let file = hook_file(layout.kind, &home.join(config));
            let mut steps: Vec<Step> = hook_step(layout.kind, wanted).into_iter().collect();
            // Claude Code keeps its switches and its AGENTS.md option beside its hooks:
            // all three edit the same bytes, in this order, for one write.
            if let Some(slot) = slots.0.remove(&file) {
                steps.push(slot.step(plan, &file));
            }
            if layout.kind == HarnessKind::ClaudeCode
                && let Some(choice) = desired.claude_agents_md
            {
                steps.push(agents_md_step(choice));
            }
            edit_config(plan, home, &file, &steps);
        }
    }
}

/// The switch files of this pass, by path, and what each must switch off.
#[derive(Default)]
struct Slots(BTreeMap<PathBuf, Slot>);

struct Slot {
    kind: HarnessKind,
    reach: Reach,
    /// Skill name → the folder the agent loads it from.
    off: BTreeMap<String, PathBuf>,
    /// For a file inside a checkout: where, and whether git tracks it.
    repo: Option<InRepo>,
}

struct InRepo {
    root: PathBuf,
    rel: String,
    tracked: bool,
}

/// The first candidate that exists, else the last (the one to create).
fn resolve(base: &Path, candidates: &[&str]) -> Option<(PathBuf, String)> {
    let found = candidates
        .iter()
        .find(|c| fs::symlink_metadata(base.join(c)).is_ok())
        .or_else(|| candidates.last())?;
    Some((base.join(found), (*found).to_owned()))
}

impl Slots {
    /// A slot for every present agent's switch file at `reach`, so Atlas's entries leave
    /// a file once nothing there is off, and `off`'s skills in each.
    fn add(
        &mut self,
        plan: &mut Plan,
        home: &Path,
        harnesses: &[&'static Layout],
        reach: Reach,
        checkout: Option<&Checkout>,
        off: &BTreeMap<HarnessKind, BTreeMap<String, PathBuf>>,
    ) {
        for layout in harnesses {
            let Some(sf) = switch_file(layout.kind, reach) else {
                continue;
            };
            let (base, render_reach) = match (sf.base, checkout) {
                (Base::Home, _) => (home, Reach::Home),
                (Base::Repo, Some(co)) => (co.root.as_path(), Reach::Project),
                (Base::Repo, None) => continue,
            };
            let Some((file, rel)) = resolve(base, sf.candidates) else {
                continue;
            };
            let slot = self.0.entry(file.clone()).or_insert_with(|| Slot {
                kind: layout.kind,
                reach: render_reach,
                off: BTreeMap::new(),
                repo: checkout.filter(|_| sf.base == Base::Repo).map(|co| InRepo {
                    root: co.root.clone(),
                    tracked: co.tracked.as_ref().is_ok_and(|t| t.covers(&rel)),
                    rel,
                }),
            });
            for (name, dir) in off.get(&layout.kind).into_iter().flatten() {
                match slot.off.get(name) {
                    Some(first) if first != dir => plan.conflicts.push(format!(
                        "skill {name} is off for {} at both {} and {}, but {} can switch off \
                         only one skill of a name; only the first is off",
                        layout.kind,
                        display(home, first),
                        display(home, dir),
                        display(home, &file)
                    )),
                    Some(_) => {}
                    None => {
                        slot.off.insert(name.clone(), dir.clone());
                    }
                }
            }
        }
    }

    /// The files inside `co` Atlas switches skills off in, relative to it, for its
    /// exclude block.
    fn in_repo(&self, co: &Checkout) -> BTreeSet<String> {
        self.0
            .values()
            .filter_map(|s| {
                s.repo
                    .as_ref()
                    .filter(|_| !s.off.is_empty())
                    .map(|r| (r, s))
            })
            .filter(|(r, _)| r.root == co.root && !r.tracked)
            .map(|(r, _)| r.rel.clone())
            .collect()
    }
}

impl Slot {
    /// The render that switches this slot's skills off, recorded in the plan.
    fn step(&self, plan: &mut Plan, file: &Path) -> Step {
        if !self.off.is_empty() {
            plan.switches.push(SwitchIntent {
                kind: self.kind,
                file: file.to_path_buf(),
                off: self.off.clone(),
            });
        }
        switch_step(self.kind, self.reach, self.off.clone())
    }
}

/// Where each global skill used in the home goes. Skills used only in some projects get
/// no home links.
fn home_skills(harnesses: &[&'static Layout], desired: &Desired, home: &Path) -> Placed {
    let mut placer = Placer::new(harnesses, Reach::Home, home);
    placer.seed(&[]);
    for (name, source) in &desired.skills {
        let placement = desired.placement(name);
        if placement.global() {
            placer.add(name, source, &placement.off, &[]);
        }
    }
    placer.placed
}

/// More entries than this in one skills folder and Atlas leaves the folder alone.
const MAX_ENTRIES: usize = 5000;

fn read_entries(dir: &Path) -> io::Result<BTreeMap<String, Entry>> {
    let mut out = BTreeMap::new();
    for (i, entry) in fs::read_dir(dir)?.enumerate() {
        if i >= MAX_ENTRIES {
            return Err(io::Error::other(format!(
                "it has more than {MAX_ENTRIES} entries"
            )));
        }
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)?;
        let item = if meta.file_type().is_symlink() {
            let target = fs::read_link(&path)?;
            if fs::metadata(&path).is_ok() {
                Entry::Link(target)
            } else {
                Entry::BrokenLink(target)
            }
        } else if meta.is_dir() {
            Entry::Dir
        } else {
            Entry::File
        };
        out.insert(name, item);
    }
    Ok(out)
}

/// What a folder of links holds now, or why Atlas leaves it alone. None: the folder
/// doesn't exist.
fn current_entries(
    dir: &Path,
    shown: &str,
    noun: &str,
) -> Result<Option<BTreeMap<String, Entry>>, String> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => Err(format!(
            "{shown} is a link to another folder; Atlas leaves it and its {noun}s alone"
        )),
        Ok(m) if !m.is_dir() => Err(format!("{shown} isn't a folder; Atlas leaves it alone")),
        Ok(_) => read_entries(dir)
            .map(Some)
            .map_err(|e| format!("can't read {shown}: {e}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("can't read {shown}: {e}")),
    }
}

/// Digests of the skill sources an exact copy was compared with, so each is read once
/// per pass.
#[derive(Default)]
pub(crate) struct Digests(BTreeMap<PathBuf, Option<String>>);

impl Digests {
    fn of(&mut self, source: &Path) -> Option<String> {
        self.0
            .entry(source.to_path_buf())
            .or_insert_with(|| tree::digest(source))
            .clone()
    }
}

/// Fingerprints the real folders in `current` that have a wanted skill's name (only
/// those, so a pass doesn't read every folder), and the sources they may be copies of.
fn fingerprint(
    dir: &Path,
    current: &mut BTreeMap<String, Entry>,
    skills: &BTreeMap<String, PathBuf>,
    digests: &mut Digests,
) -> BTreeMap<String, String> {
    let mut sources = BTreeMap::new();
    for (name, entry) in current.iter_mut() {
        let Some(source) = skills.get(name) else {
            continue;
        };
        if *entry != Entry::Dir {
            continue;
        }
        if let Some((_, digest)) = tree::exact(&dir.join(name)) {
            if let Some(d) = digests.of(source) {
                sources.insert(name.clone(), d);
            }
            *entry = Entry::DirDigest(digest);
        }
    }
    sources
}

/// Reads one folder of links and plans it by the skill-link rules (exact copies at a
/// wanted name become links). None when the folder is left alone, which is reported.
pub(crate) fn folder_plan(
    plan: &mut Plan,
    home: &Path,
    dir: &Path,
    wanted: &BTreeMap<String, PathBuf>,
    roots: &[PathBuf],
    digests: &mut Digests,
    noun: &str,
) -> Option<SkillPlan> {
    let shown = display(home, dir);
    let mut current = match current_entries(dir, &shown, noun) {
        Ok(Some(entries)) => entries,
        Ok(None) => {
            if !wanted.is_empty() {
                plan.ensure_dir(dir);
            }
            BTreeMap::new()
        }
        Err(e) => {
            plan.conflicts.push(e);
            return None;
        }
    };
    let sources = fingerprint(dir, &mut current, wanted, digests);
    Some(plan_skill_links_adopting(
        dir, &current, wanted, &sources, roots,
    ))
}

/// A link conflict in words.
pub(crate) fn link_conflict(home: &Path, shown: &str, noun: &str, c: SkillConflict) -> String {
    match c {
        SkillConflict::NotALink { name } => format!(
            "{shown}/{name} is your own folder or file, so Atlas didn't link its {noun} {name} there"
        ),
        SkillConflict::ForeignLink { name, target } => format!(
            "{shown}/{name} links to {}, which Atlas doesn't manage, so it was left alone",
            display(home, &target)
        ),
        SkillConflict::SourceOutsideRoots { name, source } => format!(
            "{noun} {name} at {} is outside Atlas's folders, so it wasn't linked",
            display(home, &source)
        ),
    }
}

/// Links `wanted` (entry name → source) into `dir` by the skill-link rules; `noun` says
/// what they are in the conflicts.
fn link_dir(
    plan: &mut Plan,
    home: &Path,
    dir: &Path,
    wanted: &BTreeMap<String, PathBuf>,
    roots: &[PathBuf],
    digests: &mut Digests,
    noun: &str,
) {
    let Some(planned) = folder_plan(plan, home, dir, wanted, roots, digests, noun) else {
        return;
    };
    let shown = display(home, dir);
    for op in planned.ops {
        plan.actions.push(match op {
            SkillOp::CreateLink { name, target } => Action::CreateLink {
                path: dir.join(name),
                target,
            },
            SkillOp::ReplaceLink { name, from, to } => Action::ReplaceLink {
                path: dir.join(name),
                from,
                to,
            },
            SkillOp::RemoveLink { name, target } => Action::RemoveLink {
                path: dir.join(name),
                target,
            },
            SkillOp::AdoptDir {
                name,
                target,
                digest,
            } => Action::AdoptDir {
                path: dir.join(name),
                target,
                digest,
            },
        });
    }
    for c in planned.conflicts {
        plan.conflicts.push(match c {
            SkillConflict::NotALink { name } => format!(
                "{shown}/{name} is your own folder or file, so Atlas didn't link its {noun} {name} there"
            ),
            SkillConflict::ForeignLink { name, target } => format!(
                "{shown}/{name} links to {}, which Atlas doesn't manage, so it was left alone",
                display(home, &target)
            ),
            SkillConflict::SourceOutsideRoots { name, source } => format!(
                "{noun} {name} at {} is outside Atlas's folders, so it wasn't linked",
                display(home, &source)
            ),
        });
    }
}

/// Removes every link into `own` from a folder Atlas no longer places skills in. Nothing
/// else there is touched or reported.
fn unlink_all(plan: &mut Plan, dir: &Path, own: &Path) {
    let Ok(Some(current)) = current_entries(dir, "", "skill") else {
        return;
    };
    let planned = plan_skill_links(dir, &current, &BTreeMap::new(), &[own.to_path_buf()]);
    for op in planned.ops {
        if let SkillOp::RemoveLink { name, target } = op {
            plan.actions.push(Action::RemoveLink {
                path: dir.join(name),
                target,
            });
        }
    }
}

/// What sits at an instruction file's path, or why Atlas won't touch it.
fn read_instruction(file: &Path) -> Result<InstructionFile, String> {
    match fs::symlink_metadata(file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(InstructionFile::Missing),
        Err(e) => Err(e.to_string()),
        Ok(m) if m.file_type().is_symlink() => fs::read_link(file)
            .map(InstructionFile::Link)
            .map_err(|e| e.to_string()),
        Ok(m) if !m.is_file() => Err("it isn't a regular file".to_owned()),
        Ok(m) if m.len() > MAX_FILE => Err("it is larger than 1 MiB".to_owned()),
        Ok(_) => fs::read_to_string(file)
            .map(InstructionFile::File)
            .map_err(|e| e.to_string()),
    }
}

/// The store's `AGENTS.md` text, read once per plan. Unreadable counts as empty, so only
/// empty files can be swapped for the link.
fn read_store_text(plan: &mut Plan, home: &Path, store: &Path) -> String {
    match read_instruction(store) {
        Ok(InstructionFile::File(text)) => text,
        Ok(_) => String::new(),
        Err(e) => {
            plan.conflicts.push(format!(
                "can't read {}: {e}; only empty instruction files will be linked",
                display(home, store)
            ));
            String::new()
        }
    }
}

fn instruction(
    plan: &mut Plan,
    home: &Path,
    kind: HarnessKind,
    file: &Path,
    (store, store_text): (&Path, &str),
    roots: &[PathBuf],
) {
    let shown = display(home, file);
    let current = match read_instruction(file) {
        Ok(c) => c,
        Err(e) => {
            plan.conflicts.push(format!("{shown} was left alone: {e}"));
            return;
        }
    };
    let action = if kind == HarnessKind::ClaudeCode {
        plan_claude_md(file, &current, store, store_text, roots, home)
    } else {
        plan_instruction_link(file, &current, store, store_text, roots)
    };
    let was = match &current {
        InstructionFile::File(text) => Some(text.clone().into_bytes()),
        _ => None,
    };
    let parent = file.parent().unwrap_or(home);
    match action {
        InstructionAction::Keep => {}
        InstructionAction::CreateLink { target } => {
            plan.ensure_dir(parent);
            plan.actions.push(Action::CreateLink {
                path: file.to_path_buf(),
                target,
            });
        }
        InstructionAction::ReplaceLink { from, to } => plan.actions.push(Action::ReplaceLink {
            path: file.to_path_buf(),
            from,
            to,
        }),
        InstructionAction::Write(bytes) => {
            plan.ensure_dir(parent);
            // Every line the old text had and the new one lacks must be in the store,
            // except Atlas's own import, which the new text replaces.
            let old = was
                .as_deref()
                .map_or_else(Default::default, String::from_utf8_lossy);
            let ours = |l: &str| is_atlas_import(l, store, roots, parent, home);
            let imports: Vec<String> = old.lines().filter(|l| ours(l)).map(str::to_owned).collect();
            let drops = !contained(&old, &String::from_utf8_lossy(&bytes), ours);
            plan.actions.push(Action::WriteInstruction {
                path: file.to_path_buf(),
                was,
                kept_in: drops.then(|| store.to_path_buf()),
                imports,
                why: if drops {
                    "its own text is all in the global instructions now, so just the import stays"
                } else {
                    "add the import of the global instructions"
                },
                bytes,
            });
        }
        InstructionAction::ReplaceFileWithLink { target } => {
            if let Some(was) = was {
                plan.actions.push(Action::FileToLink {
                    path: file.to_path_buf(),
                    was,
                    target,
                });
            }
        }
        InstructionAction::Conflict(c) => plan.conflicts.push(match c {
            InstructionConflict::UserFile => format!(
                "{shown} has text that isn't in the global instructions, so Atlas kept it and didn't link them there (Use Global instead on the hub's Instructions page adds it to them first)"
            ),
            InstructionConflict::ForeignLink { target } => format!(
                "{shown} links to {}, which Atlas doesn't manage, so it was left alone",
                display(home, &target)
            ),
            InstructionConflict::UnimportablePath => format!(
                "{shown}: the global instructions' path has a space, so Claude Code can't import it"
            ),
        }),
    }
}

/// A format over a config file's current bytes.
type Render = Box<dyn Fn(Option<&[u8]>) -> Result<Change, FormatError>>;

/// One rewrite of a config file: a format applied to the bytes the step before left.
pub(crate) struct Step {
    /// What Atlas keeps there, for messages: "hooks" or "skill switches".
    what: &'static str,
    /// Atlas has entries to put there, so a link at the path is worth reporting.
    wanted: bool,
    render: Render,
}

/// Claude Code's option for reading a project's `AGENTS.md` (decision 0029), set in
/// `~/.claude/settings.json` after Atlas's hooks and switches.
fn agents_md_step(choice: ClaudeAgentsMd) -> Step {
    Step {
        what: "Claude Code's AGENTS.md option",
        wanted: true,
        render: Box::new(move |current| claude_agents_md_setting(current, choice)),
    }
}

/// Atlas writes hooks only in these formats; other harnesses' hook files are only read.
/// Grok Build's hook files use Claude Code's shape.
fn hook_step(kind: HarnessKind, wanted: Vec<ManagedHook>) -> Option<Step> {
    let render = match kind {
        HarnessKind::ClaudeCode | HarnessKind::Grok => claude_settings,
        HarnessKind::Codex => codex_hooks,
        HarnessKind::Antigravity => antigravity_hooks,
        _ => return None,
    };
    Some(Step {
        what: "hooks",
        wanted: !wanted.is_empty(),
        render: Box::new(move |current| render(current, &wanted)),
    })
}

/// Switches `off` (skill name → the folder the agent loads it from) off in `kind`'s own
/// setting, and Atlas's other switches in that file back on. The formats find Atlas's
/// earlier entries by their marks in the file, so nothing is kept between passes.
fn switch_step(kind: HarnessKind, reach: Reach, off: BTreeMap<String, PathBuf>) -> Step {
    let names: BTreeSet<String> = off.keys().cloned().collect();
    Step {
        what: "skill switches",
        wanted: !names.is_empty(),
        render: Box::new(move |current| {
            if current.is_none() && names.is_empty() {
                return Ok(Change::Unchanged);
            }
            render_switches(kind, reach, current, &names, &BTreeSet::new(), &|n| {
                off.get(n).cloned()
            })
        }),
    }
}

/// Why a config file is written, from the parts that changed in it.
fn why(changed: &[&str]) -> &'static str {
    let has = |w: &str| changed.contains(&w);
    match (
        has("hooks"),
        has("skill switches"),
        has("Claude Code's AGENTS.md option"),
    ) {
        (true, false, false) => "update Atlas's hooks",
        (false, true, false) => "update Atlas's skill switches",
        (false, false, true) => "set Claude Code's AGENTS.md option",
        (true, true, false) => "update Atlas's hooks and skill switches",
        (true, false, true) => "update Atlas's hooks and Claude Code's AGENTS.md option",
        (false, true, true) => "update Atlas's skill switches and Claude Code's AGENTS.md option",
        _ => "update Atlas's hooks, skill switches and Claude Code's AGENTS.md option",
    }
}

/// Runs `steps` in order over one config file's bytes and plans a single write (or
/// removal) with the result, so formats sharing a file never undo each other.
pub(crate) fn edit_config(plan: &mut Plan, home: &Path, config: &Path, steps: &[Step]) {
    if steps.is_empty() {
        return;
    }
    let shown = display(home, config);
    let current = match fs::symlink_metadata(config) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            plan.conflicts.push(format!("can't read {shown}: {e}"));
            return;
        }
        Ok(m) if m.file_type().is_symlink() => {
            let wanted: Vec<&str> = steps.iter().filter(|s| s.wanted).map(|s| s.what).collect();
            if !wanted.is_empty() {
                plan.conflicts.push(format!(
                    "{shown} is a link; Atlas doesn't write {} through it",
                    wanted.join(" or ")
                ));
            }
            return;
        }
        Ok(m) if !m.is_file() || m.len() > MAX_FILE => {
            plan.conflicts
                .push(format!("{shown} isn't a small regular file; left alone"));
            return;
        }
        Ok(_) => match fs::read(config) {
            Ok(bytes) => Some(bytes),
            Err(e) => {
                plan.conflicts.push(format!("can't read {shown}: {e}"));
                return;
            }
        },
    };
    let mut bytes = current;
    let mut changed: Vec<&str> = Vec::new();
    for step in steps {
        match (step.render)(bytes.as_deref()) {
            Ok(Change::Unchanged) => {}
            Ok(Change::Write(b)) => {
                bytes = Some(b);
                changed.push(step.what);
            }
            Ok(Change::Remove) => {
                bytes = None;
                changed.push(step.what);
            }
            Err(e) => {
                plan.conflicts.push(format!("{shown} was left alone: {e}"));
                return;
            }
        }
    }
    if changed.is_empty() {
        return;
    }
    match bytes {
        Some(bytes) => {
            plan.ensure_dir(config.parent().unwrap_or(home));
            plan.actions.push(Action::WriteFile {
                path: config.to_path_buf(),
                bytes,
                why: why(&changed),
            });
        }
        None => plan.actions.push(Action::RemoveFile {
            path: config.to_path_buf(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(what: &'static str, add: &'static str) -> Step {
        Step {
            what,
            wanted: true,
            render: Box::new(move |current| {
                let mut b = current.unwrap_or_default().to_vec();
                b.extend_from_slice(add.as_bytes());
                Ok(Change::Write(b))
            }),
        }
    }

    #[test]
    fn formats_sharing_a_file_build_on_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "mine;").unwrap();
        let mut plan = Plan::default();
        edit_config(
            &mut plan,
            dir.path(),
            &file,
            &[step("hooks", "hooks;"), step("skill switches", "switches;")],
        );
        assert_eq!(
            plan.actions,
            [Action::WriteFile {
                path: file.clone(),
                bytes: b"mine;hooks;switches;".to_vec(),
                why: "update Atlas's hooks and skill switches",
            }],
            "one write, with both formats' entries and the owner's"
        );

        let unchanged = Step {
            what: "hooks",
            wanted: false,
            render: Box::new(|_| Ok(Change::Unchanged)),
        };
        let mut plan = Plan::default();
        edit_config(
            &mut plan,
            dir.path(),
            &file,
            &[unchanged, step("skill switches", "s;")],
        );
        assert!(matches!(
            &plan.actions[..],
            [Action::WriteFile {
                why: "update Atlas's skill switches",
                ..
            }]
        ));
    }

    #[test]
    fn claudes_hooks_switches_and_agents_md_option_share_one_write() {
        // The real formats, in the order plan_machine runs them on settings.json.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "{\n  \"model\": \"opus\"\n}\n").unwrap();
        let hook = ManagedHook {
            name: atlas_core::kernel::Slug::new("guard").unwrap(),
            event: atlas_core::catalog::HookEvent::PreToolUse,
            matcher: Some("Bash".into()),
            script: "/srv/guard.sh".into(),
            timeout: None,
            replaces: None,
        };
        let off = BTreeMap::from([("pdf".to_owned(), PathBuf::from("/u/.claude/skills/pdf"))]);
        let mut plan = Plan::default();
        let steps = [
            hook_step(HarnessKind::ClaudeCode, vec![hook]).unwrap(),
            switch_step(HarnessKind::ClaudeCode, Reach::Home, off),
            agents_md_step(ClaudeAgentsMd::Alongside),
        ];
        edit_config(&mut plan, dir.path(), &file, &steps);
        let [Action::WriteFile { bytes, why, .. }] = &plan.actions[..] else {
            panic!("one write: {:?}", plan.actions);
        };
        assert_eq!(
            *why,
            "update Atlas's hooks, skill switches and Claude Code's AGENTS.md option"
        );
        let v: serde_json::Value = serde_json::from_slice(bytes).unwrap();
        assert_eq!(v["model"], "opus", "the owner's setting stays");
        assert!(v["hooks"]["PreToolUse"].is_array(), "{v}");
        assert_eq!(v["skillOverrides"]["pdf"], "off", "{v}");
        assert_eq!(
            v["pluginConfigs"]["cc-plugin-agents-md@builtin"]["options"]["instructionFiles"],
            "claude-md-and-agents-md",
            "{v}"
        );

        // Applied, the same three steps have nothing left to do.
        fs::write(&file, bytes).unwrap();
        let mut again = Plan::default();
        edit_config(&mut again, dir.path(), &file, &steps);
        assert_eq!(again.actions, [], "{:?}", again.conflicts);
    }

    #[test]
    fn hosts_are_never_planned_as_agents() {
        let home = tempfile::tempdir().unwrap();
        for marker in [".t3", ".config/herdr", ".claude"] {
            fs::create_dir_all(home.path().join(marker)).unwrap();
        }
        let kinds: Vec<HarnessKind> = present(home.path()).iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [HarnessKind::ClaudeCode]);
    }

    #[test]
    fn a_failing_format_leaves_the_whole_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("settings.json");
        fs::write(&file, "{").unwrap();
        let broken = Step {
            what: "skill switches",
            wanted: true,
            render: Box::new(|_| {
                Err(FormatError::UnexpectedShape {
                    at: "settings".into(),
                    expected: "an object",
                })
            }),
        };
        let mut plan = Plan::default();
        edit_config(&mut plan, dir.path(), &file, &[step("hooks", "h"), broken]);
        assert_eq!(plan.actions, []);
        assert_eq!(plan.conflicts.len(), 1, "{:?}", plan.conflicts);
    }
}
