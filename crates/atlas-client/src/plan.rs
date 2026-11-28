//! The plan for one login: which links and files each present harness needs. Reading
//! the current state happens here; deciding is `atlas-harness`'s pure functions.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use atlas_core::kernel::HarnessKind;
use atlas_harness::hooks::{ManagedHook, antigravity_hooks, claude_settings, codex_hooks};
use atlas_harness::instructions::{
    InstructionAction, InstructionConflict, InstructionFile, claude_agents_md_setting,
    plan_claude_md, plan_instruction_link,
};
use atlas_harness::skills::{Entry, SkillConflict, SkillOp, plan_skill_links};
use atlas_harness::{Change, Layout, layouts};

use crate::desired::Desired;
use crate::display;

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
    /// An instruction file planned from `was` (`None`: there was none) gets `bytes`. When
    /// the new text drops some of `was` (Claude's `CLAUDE.md` becoming just the import),
    /// `kept_in` names the store file that must still hold every dropped line when this
    /// is applied.
    WriteInstruction {
        path: PathBuf,
        was: Option<Vec<u8>>,
        bytes: Vec<u8>,
        kept_in: Option<PathBuf>,
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
                format!("remove {} (Atlas's hooks were all it held)", d(path))
            }
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

/// Plans every present harness: skill and subagent links, instruction files and hook
/// configs.
#[must_use]
pub fn plan(home: &Path, desired: &Desired, managed_roots: &[PathBuf]) -> Plan {
    let mut plan = Plan::default();
    let harnesses = present(home);
    // Each harness needs the links in one folder it reads. A harness that already reads a
    // folder getting them (OpenCode reads Claude's and Codex's) gets nothing more, and a
    // harness's other folders are left alone, or it would see every skill twice (Codex
    // reads both ~/.agents/skills and ~/.codex/skills).
    let mut linked: BTreeSet<&str> = BTreeSet::new();
    for layout in &harnesses {
        if layout.skill_dirs.iter().any(|d| linked.contains(d)) {
            continue;
        }
        if let Some(dir) = layout.skill_dirs.first() {
            linked.insert(dir);
            link_dir(
                &mut plan,
                home,
                &home.join(dir),
                &desired.skills,
                managed_roots,
                "skill",
            );
        }
    }
    let claude = harnesses.iter().any(|l| l.kind == HarnessKind::ClaudeCode);
    let store_text = desired
        .instructions
        .as_deref()
        .map(|s| read_store_text(&mut plan, home, s));
    for layout in &harnesses {
        // Subagents: only Claude Code's format is known so far (HARNESSES.md).
        if let Some(dir) = layout.agent_dir {
            link_dir(
                &mut plan,
                home,
                &home.join(dir),
                &desired.agents,
                managed_roots,
                "subagent",
            );
        }
        if let (Some(store), Some(text)) = (&desired.instructions, &store_text) {
            for file in layout.instruction_files {
                instruction(
                    &mut plan,
                    home,
                    layout.kind,
                    &home.join(file),
                    (store, text),
                    managed_roots,
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
            hooks(
                &mut plan,
                home,
                layout.kind,
                &hook_file(layout.kind, &home.join(config)),
                &wanted,
            );
        }
    }
    if claude && let Some(choice) = desired.claude_agents_md {
        claude_setting(&mut plan, home, choice);
    }
    plan
}

fn read_entries(dir: &Path) -> io::Result<BTreeMap<String, Entry>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(dir)? {
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

/// Links `wanted` (entry name → source) into `dir` by the skill-link rules; `noun` says
/// what they are in the conflicts.
fn link_dir(
    plan: &mut Plan,
    home: &Path,
    dir: &Path,
    wanted: &BTreeMap<String, PathBuf>,
    roots: &[PathBuf],
    noun: &str,
) {
    let shown = display(home, dir);
    let current = match fs::symlink_metadata(dir) {
        Ok(m) if m.file_type().is_symlink() => {
            plan.conflicts.push(format!(
                "{shown} is a link to another folder; Atlas leaves it and its {noun}s alone"
            ));
            return;
        }
        Ok(m) if !m.is_dir() => {
            plan.conflicts
                .push(format!("{shown} isn't a folder; Atlas leaves it alone"));
            return;
        }
        Ok(_) => match read_entries(dir) {
            Ok(entries) => entries,
            Err(e) => {
                plan.conflicts.push(format!("can't read {shown}: {e}"));
                return;
            }
        },
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            if !wanted.is_empty() {
                plan.ensure_dir(dir);
            }
            BTreeMap::new()
        }
        Err(e) => {
            plan.conflicts.push(format!("can't read {shown}: {e}"));
            return;
        }
    };
    let skills = plan_skill_links(dir, &current, wanted, roots);
    for op in skills.ops {
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
    for c in skills.conflicts {
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
            // Every line the old text had and the new one lacks must be in the store.
            let drops = was.as_ref().is_some_and(|w| {
                let w = String::from_utf8_lossy(w);
                let b = String::from_utf8_lossy(&bytes);
                !atlas_harness::instructions::contained(&w, &b, |_| false)
            });
            plan.actions.push(Action::WriteInstruction {
                path: file.to_path_buf(),
                was,
                kept_in: drops.then(|| store.to_path_buf()),
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

/// Claude Code's AGENTS.md option in `~/.claude/settings.json`. The hooks (and skill
/// switches) may already have planned a write of that file; the setting is rendered over
/// those bytes, so one write carries both.
fn claude_setting(plan: &mut Plan, home: &Path, choice: atlas_core::catalog::ClaudeAgentsMd) {
    let path = home.join(".claude/settings.json");
    let shown = display(home, &path);
    let pending = plan.actions.iter().position(|a| {
        matches!(a, Action::WriteFile { path: p, .. } | Action::RemoveFile { path: p } if *p == path)
    });
    let current: Option<Vec<u8>> = match pending.map(|i| &plan.actions[i]) {
        Some(Action::WriteFile { bytes, .. }) => Some(bytes.clone()),
        Some(_) => None,
        None => match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                plan.conflicts.push(format!("can't read {shown}: {e}"));
                return;
            }
            Ok(m) if m.file_type().is_symlink() => {
                plan.conflicts.push(format!(
                    "{shown} is a link; Atlas doesn't set Claude Code's AGENTS.md option through it"
                ));
                return;
            }
            Ok(m) if !m.is_file() || m.len() > MAX_FILE => {
                plan.conflicts
                    .push(format!("{shown} isn't a small regular file; left alone"));
                return;
            }
            Ok(_) => match fs::read(&path) {
                Ok(bytes) => Some(bytes),
                Err(e) => {
                    plan.conflicts.push(format!("can't read {shown}: {e}"));
                    return;
                }
            },
        },
    };
    let bytes = match claude_agents_md_setting(current.as_deref(), choice) {
        Ok(Change::Write(bytes)) => bytes,
        Ok(_) => return,
        Err(e) => {
            plan.conflicts.push(format!(
                "{shown}: Claude Code's AGENTS.md option wasn't set: {e}"
            ));
            return;
        }
    };
    if let Some(i) = pending {
        plan.actions[i] = Action::WriteFile {
            path,
            bytes,
            why: "update Atlas's hooks and Claude Code's AGENTS.md option",
        };
    } else {
        plan.ensure_dir(path.parent().unwrap_or(home));
        plan.actions.push(Action::WriteFile {
            path,
            bytes,
            why: "set Claude Code's AGENTS.md option",
        });
    }
}

fn hooks(plan: &mut Plan, home: &Path, kind: HarnessKind, config: &Path, wanted: &[ManagedHook]) {
    // Atlas writes hooks only in these formats; other harnesses' hook files are only read.
    // Grok Build's hook files use Claude Code's shape.
    let render = match kind {
        HarnessKind::ClaudeCode | HarnessKind::Grok => claude_settings,
        HarnessKind::Codex => codex_hooks,
        HarnessKind::Antigravity => antigravity_hooks,
        _ => return,
    };
    let shown = display(home, config);
    let current = match fs::symlink_metadata(config) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => {
            plan.conflicts.push(format!("can't read {shown}: {e}"));
            return;
        }
        Ok(m) if m.file_type().is_symlink() => {
            if !wanted.is_empty() {
                plan.conflicts.push(format!(
                    "{shown} is a link; Atlas doesn't write hooks through it"
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
    match render(current.as_deref(), wanted) {
        Ok(Change::Unchanged) => {}
        Ok(Change::Write(bytes)) => {
            plan.ensure_dir(config.parent().unwrap_or(home));
            plan.actions.push(Action::WriteFile {
                path: config.to_path_buf(),
                bytes,
                why: "update Atlas's hooks",
            });
        }
        Ok(Change::Remove) => plan.actions.push(Action::RemoveFile {
            path: config.to_path_buf(),
        }),
        Err(e) => plan.conflicts.push(format!("{shown} was left alone: {e}")),
    }
}
