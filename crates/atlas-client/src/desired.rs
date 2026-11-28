//! What should be installed for this login, read from the local copies: global skills,
//! where each goes (`atlas/placements.toml`), the global instructions, the global hooks
//! and Claude Code's subagents.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::catalog::{ClaudeAgentsMd, HookManifest, PLACEMENTS_FILE, Placement, Placements};
use atlas_core::kernel::{HarnessKind, Slug};
use atlas_fs::library::LibraryError;
use atlas_harness::hooks::ManagedHook;

const MAX_HOOK_TOML: u64 = 64 * 1024;

/// One hook and the harnesses it is limited to (None: all that have its event).
#[derive(Clone, Debug)]
pub struct HookSpec {
    pub hook: ManagedHook,
    pub harnesses: Option<Vec<HarnessKind>>,
}

#[derive(Clone, Debug, Default)]
pub struct Desired {
    /// Skill name → its folder in a local copy.
    pub skills: BTreeMap<String, PathBuf>,
    /// Where each skill goes. Only exceptions are listed.
    pub placements: Placements,
    /// `atlas/placements.toml` couldn't be read. No skill link or switch changes then, so
    /// a typo can't strip every skill; the reason is in `problems`.
    pub placements_invalid: bool,
    /// The global `AGENTS.md` in the knowledge copy.
    pub instructions: Option<PathBuf>,
    /// The owner's choice for when Claude Code reads a project's `AGENTS.md`; `None`
    /// leaves Claude Code's setting as it is.
    pub claude_agents_md: Option<ClaudeAgentsMd>,
    pub hooks: Vec<HookSpec>,
    /// Subagent file name (`<name>.md`) → the file in the knowledge copy.
    pub agents: BTreeMap<String, PathBuf>,
    pub problems: Vec<String>,
}

impl Desired {
    /// Where a global skill goes; the default when it isn't listed or isn't a valid name.
    #[must_use]
    pub fn placement(&self, name: &str) -> Placement {
        Slug::new(name)
            .ok()
            .and_then(|s| self.placements.global.get(&s).cloned())
            .unwrap_or_default()
    }

    /// The hooks one harness should get.
    #[must_use]
    pub fn hooks_for(&self, kind: HarnessKind) -> Vec<ManagedHook> {
        self.hooks
            .iter()
            .filter(|h| h.harnesses.as_ref().is_none_or(|l| l.contains(&kind)))
            .map(|h| h.hook.clone())
            .collect()
    }
}

/// Folders under `dir` that hold a `SKILL.md`, by name. Names that aren't slugs are
/// reported and skipped.
pub(crate) fn skills_in(dir: &Path, problems: &mut Vec<String>) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let path = entry.path();
        // Only real folders: a link in a repo could point anywhere.
        let is_dir = fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir());
        if name.starts_with('.') || !is_dir || !path.join("SKILL.md").is_file() {
            continue;
        }
        if Slug::new(name.as_str()).is_err() {
            problems.push(format!(
                "skill folder {name:?} skipped: its name isn't lowercase-with-hyphens"
            ));
            continue;
        }
        out.insert(name, path);
    }
    out
}

/// Regular `<name>.md` files directly under `dir`, by file name. Hidden files, links and
/// names that aren't slugs are skipped, the last two reported.
fn agents_in(dir: &Path, problems: &mut Vec<String>) -> BTreeMap<String, PathBuf> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(stem) = name.strip_suffix(".md") else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        // Only real files: a link in a repo could point anywhere.
        if !fs::symlink_metadata(&path).is_ok_and(|m| m.is_file()) {
            problems.push(format!("subagent {name} skipped: it isn't a regular file"));
            continue;
        }
        if Slug::new(stem).is_err() {
            problems.push(format!(
                "subagent {name} skipped: its name isn't lowercase-with-hyphens"
            ));
            continue;
        }
        out.insert(name, path);
    }
    out
}

fn hook(dir: &Path, name: &str) -> Result<HookSpec, String> {
    let slug = Slug::new(name).map_err(|e| e.to_string())?;
    let file = dir.join("hook.toml");
    let meta = fs::symlink_metadata(&file).map_err(|e| format!("no hook.toml ({e})"))?;
    if !meta.is_file() || meta.len() > MAX_HOOK_TOML {
        return Err("hook.toml isn't a small regular file".to_owned());
    }
    let text = fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let manifest: HookManifest = toml::from_str(&text).map_err(|e| e.message().to_owned())?;
    let run = manifest.run.as_str();
    if run.is_empty() || run.contains('/') || run.starts_with('.') {
        return Err(format!(
            "`run = {run:?}` must name a file in the hook's folder"
        ));
    }
    let script = dir.join(run);
    if !fs::symlink_metadata(&script).is_ok_and(|m| m.is_file()) {
        return Err(format!("its script {run} is missing"));
    }
    Ok(HookSpec {
        hook: ManagedHook {
            name: slug,
            event: manifest.event,
            matcher: manifest.matcher,
            script: script.to_string_lossy().into_owned(),
            timeout: manifest.timeout,
        },
        harnesses: manifest.harnesses,
    })
}

/// Reads the desired state from the knowledge copy and the public skills copy.
/// Knowledge wins when both have a skill of the same name.
#[must_use]
pub fn read(knowledge: Option<&Path>, public_skills: Option<&Path>) -> Desired {
    let mut d = Desired::default();
    let private = knowledge.map_or_else(BTreeMap::new, |k| {
        skills_in(&k.join("skills"), &mut d.problems)
    });
    if let Some(public) = public_skills {
        for (name, path) in skills_in(public, &mut d.problems) {
            if private.contains_key(&name) {
                d.problems.push(format!(
                    "skill {name} is in both the knowledge repo and the public skills repo; using the knowledge repo's"
                ));
            } else {
                d.skills.insert(name, path);
            }
        }
    }
    d.skills.extend(private);

    let Some(knowledge) = knowledge else {
        return d;
    };
    // One reader for the whole file, skills and instructions alike: only a missing file
    // means the defaults; anything else wrong changes no skill link, switch or setting.
    match atlas_fs::library::read_placements(knowledge) {
        Ok((placements, _)) => {
            d.claude_agents_md = placements.instructions.claude_reads_agents_md;
            d.placements = placements;
        }
        Err(e) => {
            d.placements_invalid = true;
            let why = match e {
                LibraryError::Invalid(_) | LibraryError::Symlink(_) => e.to_string(),
                other => format!("{PLACEMENTS_FILE} can't be read: {other}"),
            };
            d.problems.push(format!(
                "{why}; no skill links, switches or Claude Code setting change until it is fixed"
            ));
        }
    }
    let agents = knowledge.join("AGENTS.md");
    if fs::symlink_metadata(&agents).is_ok_and(|m| m.is_file()) {
        d.instructions = Some(agents);
    }
    d.agents = agents_in(&knowledge.join("agents"), &mut d.problems);
    let hooks_dir = knowledge.join("hooks");
    let mut entries: Vec<_> = fs::read_dir(&hooks_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| fs::symlink_metadata(e.path()).is_ok_and(|m| m.is_dir()))
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| !n.starts_with('.'))
        .collect();
    entries.sort();
    for name in entries {
        match hook(&hooks_dir.join(&name), &name) {
            Ok(spec) => d.hooks.push(spec),
            Err(e) => d.problems.push(format!("hook {name} skipped: {e}")),
        }
    }
    d
}
