//! The plan for one login: which links and files each present harness needs. Reading
//! the current state happens here; deciding is `atlas-harness`'s pure functions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use atlas_core::kernel::HarnessKind;
use atlas_harness::hooks::{ManagedHook, antigravity_hooks, claude_settings, codex_hooks};
use atlas_harness::instructions::{
    InstructionAction, InstructionConflict, InstructionFile, plan_claude_md, plan_instruction_link,
};
use atlas_harness::placement::Reach;
use atlas_harness::skills::{Entry, SkillConflict, SkillOp, plan_skill_links};
use atlas_harness::switches::{render_switches, switch_file};
use atlas_harness::{Change, FormatError, Layout, layouts};

use crate::desired::Desired;
use crate::display;
use crate::place::{Placed, Placer};

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
            Self::WriteFile { path, why, .. } => format!("write {} ({why})", d(path)),
            Self::RemoveFile { path } => {
                format!("remove {} (Atlas's entries were all it held)", d(path))
            }
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
    fn ensure_dir(&mut self, dir: &Path) {
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

/// The harnesses set up for this login. T3 Code reads the others' files.
#[must_use]
pub fn present(home: &Path) -> Vec<&'static Layout> {
    layouts()
        .iter()
        .filter(|l| l.kind != HarnessKind::T3 && home.join(l.marker).exists())
        .collect()
}

/// Plans every present harness: skill links, skill switches, instruction files and hook
/// configs. Links into `managed_roots` are Atlas's; the first is its own local copies,
/// which only atlasd links into.
#[must_use]
pub fn plan(home: &Path, desired: &Desired, managed_roots: &[PathBuf]) -> Plan {
    let mut plan = Plan::default();
    let harnesses = present(home);
    let managed = managed_roots;
    let mut off = BTreeMap::new();
    if !desired.placements_invalid {
        let placed = home_skills(&harnesses, desired, home);
        for (dir, skills) in &placed.folders {
            skill_dir(&mut plan, home, &home.join(dir), skills, managed);
        }
        // A folder no skill goes in any more loses the links Atlas made there.
        let mut seen: BTreeSet<&str> = placed.folders.iter().map(|(f, _)| *f).collect();
        for dir in harnesses.iter().flat_map(|l| l.skill_dirs) {
            if seen.insert(dir)
                && let Some(own) = managed.first()
            {
                unlink_all(&mut plan, &home.join(dir), own);
            }
        }
        plan.conflicts.extend(placed.stuck);
        off = placed.off;
    }
    let claude = harnesses.iter().any(|l| l.kind == HarnessKind::ClaudeCode);
    let mut switched = BTreeSet::new();
    for layout in &harnesses {
        if let Some(store) = &desired.instructions {
            for file in layout.instruction_files {
                instruction(
                    &mut plan,
                    home,
                    layout.kind,
                    &home.join(file),
                    store,
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
            // Claude Code keeps its switches beside its hooks: both edit the same bytes.
            let own_file = switch_file(layout.kind, Reach::Home).map(|f| home.join(f));
            if !desired.placements_invalid && own_file.as_deref() == Some(file.as_path()) {
                steps.push(switch_step(
                    &mut plan,
                    layout.kind,
                    Reach::Home,
                    &file,
                    off.get(&layout.kind),
                ));
                switched.insert(layout.kind);
            }
            edit_config(&mut plan, home, &file, &steps);
        }
    }
    if !desired.placements_invalid {
        for layout in &harnesses {
            if let Some(file) = switch_file(layout.kind, Reach::Home)
                && !switched.contains(&layout.kind)
            {
                let file = home.join(file);
                let step = switch_step(
                    &mut plan,
                    layout.kind,
                    Reach::Home,
                    &file,
                    off.get(&layout.kind),
                );
                edit_config(&mut plan, home, &file, &[step]);
            }
        }
    }
    plan
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

/// What a skills folder holds now, or why Atlas leaves it alone. None: the folder
/// doesn't exist.
fn current_entries(dir: &Path, shown: &str) -> Result<Option<BTreeMap<String, Entry>>, String> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => Err(format!(
            "{shown} is a link to another folder; Atlas leaves it and its skills alone"
        )),
        Ok(m) if !m.is_dir() => Err(format!("{shown} isn't a folder; Atlas leaves it alone")),
        Ok(_) => read_entries(dir)
            .map(Some)
            .map_err(|e| format!("can't read {shown}: {e}")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("can't read {shown}: {e}")),
    }
}

fn skill_dir(
    plan: &mut Plan,
    home: &Path,
    dir: &Path,
    skills: &BTreeMap<String, PathBuf>,
    roots: &[PathBuf],
) {
    let shown = display(home, dir);
    let current = match current_entries(dir, &shown) {
        Ok(Some(entries)) => entries,
        Ok(None) => {
            if !skills.is_empty() {
                plan.ensure_dir(dir);
            }
            BTreeMap::new()
        }
        Err(e) => {
            plan.conflicts.push(e);
            return;
        }
    };
    let planned = plan_skill_links(dir, &current, skills, roots);
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
        });
    }
    for c in planned.conflicts {
        plan.conflicts.push(match c {
            SkillConflict::NotALink { name } => format!(
                "{shown}/{name} is your own folder or file, so Atlas didn't link its skill {name} there"
            ),
            SkillConflict::ForeignLink { name, target } => format!(
                "{shown}/{name} links to {}, which Atlas doesn't manage, so it was left alone",
                display(home, &target)
            ),
            SkillConflict::SourceOutsideRoots { name, source } => format!(
                "skill {name} at {} is outside Atlas's folders, so it wasn't linked",
                display(home, &source)
            ),
        });
    }
}

/// Removes every link into `own` from a folder Atlas no longer places skills in. Nothing
/// else there is touched or reported.
fn unlink_all(plan: &mut Plan, dir: &Path, own: &Path) {
    let Ok(Some(current)) = current_entries(dir, "") else {
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

fn instruction(
    plan: &mut Plan,
    home: &Path,
    kind: HarnessKind,
    file: &Path,
    store: &Path,
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
        plan_claude_md(file, &current, store, roots, home)
    } else {
        plan_instruction_link(file, &current, store, roots)
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
            plan.actions.push(Action::WriteFile {
                path: file.to_path_buf(),
                bytes,
                why: "add the import of the global instructions",
            });
        }
        InstructionAction::Conflict(c) => plan.conflicts.push(match c {
            InstructionConflict::UserFile => format!(
                "{shown} is your own file, so Atlas didn't link the global instructions there"
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
struct Step {
    /// What Atlas keeps there, for messages: "hooks" or "skill switches".
    what: &'static str,
    /// Atlas has entries to put there, so a link at the path is worth reporting.
    wanted: bool,
    render: Render,
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
/// setting at `file`, and Atlas's other switches there back on.
fn switch_step(
    plan: &mut Plan,
    kind: HarnessKind,
    reach: Reach,
    file: &Path,
    off: Option<&BTreeMap<String, PathBuf>>,
) -> Step {
    if let Some(off) = off.filter(|o| !o.is_empty()) {
        plan.switches.push(SwitchIntent {
            kind,
            file: file.to_path_buf(),
            off: off.clone(),
        });
    }
    let names: BTreeSet<String> = off.into_iter().flat_map(|o| o.keys().cloned()).collect();
    // `render_switches` hands each lookup a borrow tied to the name it is given, so only
    // 'static paths fit it. atlasd runs one pass per process, so the few folders of skills
    // that are switched off are leaked rather than changing that signature here.
    let dirs: BTreeMap<String, &'static Path> = off
        .into_iter()
        .flatten()
        .map(|(name, dir)| {
            let leaked: &'static Path = Box::leak(dir.clone().into_boxed_path());
            (name.clone(), leaked)
        })
        .collect();
    Step {
        what: "skill switches",
        wanted: !names.is_empty(),
        render: Box::new(move |current| {
            if current.is_none() && names.is_empty() {
                return Ok(Change::Unchanged);
            }
            render_switches(kind, reach, current, &names, &|n| dirs.get(n).copied())
        }),
    }
}

/// Runs `steps` in order over one config file's bytes and plans a single write (or
/// removal) with the result, so formats sharing a file never undo each other.
fn edit_config(plan: &mut Plan, home: &Path, config: &Path, steps: &[Step]) {
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
                why: match changed.as_slice() {
                    ["hooks"] => "update Atlas's hooks",
                    ["skill switches"] => "update Atlas's skill switches",
                    _ => "update Atlas's hooks and skill switches",
                },
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
