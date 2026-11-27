use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::catalog::SkillHome;
use atlas_core::distribution::{
    FoundHook, HarnessView, InstalledSkill, InstructionLink, InstructionView, SkillDir,
};
use atlas_core::kernel::Slug;
use serde_json::Value;

use crate::paths::Sources;

const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

/// Looks at what every harness of this login currently sees. Reads link targets and the
/// harnesses' own config files; never follows a link to read what's behind it.
#[must_use]
pub fn scan_harnesses(sources: &Sources) -> Vec<HarnessView> {
    let homes = SkillHomes::new(sources);
    atlas_harness::layouts()
        .iter()
        .map(|layout| {
            let home = &sources.home;
            HarnessView {
                kind: layout.kind,
                present: home.join(layout.marker).exists(),
                skill_dirs: layout
                    .skill_dirs
                    .iter()
                    .map(|d| scan_skill_dir(sources, &homes, &home.join(d)))
                    .collect(),
                instructions: layout
                    .instruction_files
                    .iter()
                    .map(|f| scan_instruction(sources, &home.join(f)))
                    .collect(),
                hook_config: layout.hook_config.map(|c| sources.display(&home.join(c))),
                hooks: layout
                    .hook_config
                    .map_or_else(Vec::new, |c| read_hooks(&home.join(c))),
            }
        })
        .collect()
}

/// The canonical skill folders of the store, to recognise links into them.
struct SkillHomes(Vec<(PathBuf, SkillHome)>);

impl SkillHomes {
    fn new(sources: &Sources) -> Self {
        let mut homes = Vec::new();
        let mut add = |path: PathBuf, home: SkillHome| {
            if let Ok(canonical) = fs::canonicalize(path) {
                homes.push((canonical, home));
            }
        };
        add(sources.knowledge.join("skills"), SkillHome::Private);
        if let Some(public) = &sources.public_skills {
            add(public.clone(), SkillHome::Public);
        }
        for p in &sources.projects {
            add(
                p.root.join(".agents/skills"),
                SkillHome::Project(p.name.clone()),
            );
        }
        Self(homes)
    }

    /// Which store home a resolved skill folder belongs to, if any.
    fn classify(&self, resolved: &Path) -> Option<(SkillHome, Slug)> {
        let parent = resolved.parent()?;
        let name = Slug::new(resolved.file_name()?.to_str()?).ok()?;
        self.0
            .iter()
            .find(|(root, _)| root == parent)
            .map(|(_, home)| (home.clone(), name))
    }
}

fn scan_skill_dir(sources: &Sources, homes: &SkillHomes, dir: &Path) -> SkillDir {
    let mut entries = Vec::new();
    let read = fs::read_dir(dir);
    let exists = read.is_ok();
    for entry in read.into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        let item = if meta.file_type().is_symlink() {
            let target =
                fs::read_link(&path).map_or_else(|_| "?".to_owned(), |t| sources.display(&t));
            match fs::canonicalize(&path) {
                Err(_) => InstalledSkill::Broken { name, target },
                Ok(resolved) => match homes.classify(&resolved) {
                    Some((home, slug)) => InstalledSkill::FromAtlas { home, name: slug },
                    None => InstalledSkill::Outside {
                        name,
                        detail: format!("link to {target}"),
                    },
                },
            }
        } else if meta.is_dir() {
            InstalledSkill::Outside {
                name,
                detail: "its own folder".to_owned(),
            }
        } else {
            continue;
        };
        entries.push(item);
    }
    entries.sort_by(|a, b| a.name().cmp(b.name()));
    SkillDir {
        path: sources.display(dir),
        exists,
        entries,
    }
}

fn scan_instruction(sources: &Sources, file: &Path) -> InstructionView {
    let store = fs::canonicalize(sources.knowledge.join("AGENTS.md")).ok();
    let link = match fs::symlink_metadata(file) {
        Err(_) => InstructionLink::Missing,
        Ok(meta) if meta.file_type().is_symlink() => {
            if fs::canonicalize(file).is_ok_and(|c| Some(c) == store) {
                InstructionLink::Linked
            } else {
                InstructionLink::Separate
            }
        }
        Ok(meta) if meta.len() > MAX_CONFIG_BYTES => InstructionLink::Separate,
        Ok(_) => {
            let text = fs::read_to_string(file).unwrap_or_default();
            if imports(sources, &text, store.as_deref()) {
                InstructionLink::Imports
            } else {
                InstructionLink::Separate
            }
        }
    };
    InstructionView {
        path: sources.display(file),
        link,
    }
}

/// Whether a file has an `@path` import line that resolves to the store's instructions.
fn imports(sources: &Sources, text: &str, store: Option<&Path>) -> bool {
    let Some(store) = store else { return false };
    text.lines()
        .filter_map(|l| l.trim().strip_prefix('@'))
        .any(|target| {
            let path = target
                .strip_prefix("~/")
                .map_or_else(|| PathBuf::from(target), |rest| sources.home.join(rest));
            fs::canonicalize(path).ok().as_deref() == Some(store)
        })
}

/// Reads hooks from a Claude-style config: `{"hooks": {Event: [{matcher, hooks: [..]}]}}`.
fn read_hooks(config: &Path) -> Vec<FoundHook> {
    let too_big = fs::metadata(config).map_or(true, |m| m.len() > MAX_CONFIG_BYTES);
    if too_big {
        return Vec::new();
    }
    let Ok(text) = fs::read_to_string(config) else {
        return Vec::new();
    };
    let Ok(json) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    let Some(events) = json.get("hooks").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (event, groups) in events {
        for group in groups.as_array().into_iter().flatten() {
            let matcher = group
                .get("matcher")
                .and_then(Value::as_str)
                .filter(|m| !m.is_empty())
                .map(str::to_owned);
            for hook in group
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let Some(command) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                found.push(FoundHook {
                    event: event.clone(),
                    matcher: matcher.clone(),
                    command: command.to_owned(),
                    timeout: hook.get("timeout").and_then(Value::as_u64),
                });
            }
        }
    }
    found
}
