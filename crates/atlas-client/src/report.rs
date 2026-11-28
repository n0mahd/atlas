//! What each harness looks like after a pass, for the hub's Machines screen.

use std::fs;
use std::path::Path;

use atlas_core::distribution::{
    HarnessReport, InstalledSkill, InstructionLink, OutsideKind, OutsideSkill,
};
use atlas_core::kernel::HarnessKind;
use atlas_fs::Sources;
use atlas_harness::hooks::managed_name;
use serde_json::Value;

use crate::plan::hook_file;
use crate::sources::key;

const MAX_FILE: u64 = 1024 * 1024;

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// Atlas's hooks in a harness's hook config, as written there now.
fn managed_hooks(kind: HarnessKind, config: &Path) -> u32 {
    let small = fs::symlink_metadata(config).is_ok_and(|m| m.is_file() && m.len() <= MAX_FILE);
    let Some(json) = small
        .then(|| fs::read(config).ok())
        .flatten()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return 0;
    };
    if kind == HarnessKind::Antigravity {
        return json.as_object().map_or(0, |o| {
            count(o.keys().filter(|k| k.starts_with("atlas:")).count())
        });
    }
    let handlers = json
        .get("hooks")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|events| events.values())
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|group| group.get("hooks").and_then(Value::as_array))
        .flatten()
        .filter_map(|h| h.get("command").and_then(Value::as_str))
        .filter(|c| managed_name(c).is_some())
        .count();
    count(handlers)
}

/// Subagent links in `dir` that point into Atlas's copies and resolve.
fn linked_agents(dir: &Path, sources_dir: &Path) -> u32 {
    let links = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| fs::metadata(p).is_ok())
        .filter_map(|p| fs::read_link(&p).ok().map(|t| dir.join(t)))
        .filter(|t| t.starts_with(sources_dir))
        .count();
    count(links)
}

/// One report per harness layout, counted from the files as they are now.
#[must_use]
pub fn harnesses(home: &Path, sources_dir: &Path) -> Vec<HarnessReport> {
    let public = sources_dir.join(key(&atlas_core::release::SourceId::PublicSkills));
    let sources = Sources {
        home: home.to_path_buf(),
        knowledge: sources_dir.join(key(&atlas_core::release::SourceId::Knowledge)),
        public_skills: public.is_dir().then_some(public),
        projects: Vec::new(),
    };
    let layouts = atlas_harness::layouts();
    atlas_fs::scan_harnesses(&sources)
        .into_iter()
        .zip(layouts)
        .map(|(view, layout)| {
            let entries = view.skill_dirs.iter().flat_map(|d| d.entries.iter());
            let (mut atlas, mut outside, mut broken) = (0, 0, 0);
            for e in entries {
                match e {
                    InstalledSkill::FromAtlas { .. } => atlas += 1,
                    InstalledSkill::Outside { .. } => outside += 1,
                    InstalledSkill::Broken { .. } => broken += 1,
                }
            }
            let instructions = match view.instructions.first().map(|i| &i.link) {
                Some(InstructionLink::Linked) => "linked",
                Some(InstructionLink::Imports) => "imports",
                Some(InstructionLink::Separate) => "separate",
                Some(InstructionLink::Missing) | None => "missing",
            };
            HarnessReport {
                kind: view.kind,
                present: view.present,
                skills_from_atlas: atlas,
                skills_outside: outside,
                broken,
                instructions: instructions.to_owned(),
                hooks_managed: layout.hook_config.map_or(0, |c| {
                    managed_hooks(view.kind, &hook_file(view.kind, &home.join(c)))
                }),
                agents_from_atlas: layout
                    .agent_dir
                    .map_or(0, |d| linked_agents(&home.join(d), sources_dir)),
            }
        })
        .collect()
}

/// At most this many outside skills are reported (and fingerprinted) per pass.
pub const MAX_OUTSIDE: usize = 300;

/// The skills in every agent's home skill folders that aren't links into `managed` (Atlas's
/// copies and adopted folders): own folders with their digest, links elsewhere, broken
/// links. Hidden entries are skipped; links are read, never followed to write anything.
#[must_use]
pub fn outside(home: &Path, managed: &[std::path::PathBuf]) -> Vec<OutsideSkill> {
    let managed: Vec<std::path::PathBuf> = managed
        .iter()
        .filter_map(|r| fs::canonicalize(r).ok())
        .collect();
    let mut dirs: Vec<&str> = atlas_harness::layouts()
        .iter()
        .flat_map(|l| l.skill_dirs.iter().copied())
        .collect();
    dirs.sort_unstable();
    dirs.dedup();
    let mut out = Vec::new();
    'dirs: for dir in dirs {
        let dir = home.join(dir);
        if !fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut names: Vec<String> = entries
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| !n.starts_with('.'))
            .take(MAX_OUTSIDE * 4)
            .collect();
        names.sort();
        for name in names {
            if out.len() >= MAX_OUTSIDE {
                break 'dirs;
            }
            let path = dir.join(&name);
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            let shown = crate::display(home, &path);
            let item = if meta.file_type().is_symlink() {
                let target = fs::read_link(&path).ok().map(|t| crate::display(home, &t));
                match fs::canonicalize(&path) {
                    Err(_) => OutsideSkill {
                        name,
                        path: shown,
                        kind: OutsideKind::Broken,
                        target,
                        digest: None,
                    },
                    Ok(resolved) if managed.iter().any(|r| resolved.starts_with(r)) => continue,
                    Ok(resolved) => OutsideSkill {
                        name,
                        path: shown,
                        kind: OutsideKind::Link,
                        target,
                        digest: atlas_fs::library::read_tree(&resolved)
                            .ok()
                            .and_then(|t| t.exact_digest()),
                    },
                }
            } else if meta.is_dir() {
                OutsideSkill {
                    name,
                    path: shown,
                    kind: OutsideKind::Folder,
                    target: None,
                    digest: crate::tree::exact(&path).map(|(_, d)| d),
                }
            } else {
                continue;
            };
            out.push(item);
        }
    }
    out
}
