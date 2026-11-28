//! What should be installed for this login, read from the local copies: global skills,
//! the global instructions, the global hooks and Claude Code's subagents.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use atlas_core::catalog::HookManifest;
use atlas_core::kernel::{HarnessKind, Slug};
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
    /// The global `AGENTS.md` in the knowledge copy.
    pub instructions: Option<PathBuf>,
    pub hooks: Vec<HookSpec>,
    /// Subagent file name (`<name>.md`) → the file in the knowledge copy.
    pub agents: BTreeMap<String, PathBuf>,
    pub problems: Vec<String>,
}

impl Desired {
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
fn skills_in(dir: &Path, problems: &mut Vec<String>) -> BTreeMap<String, PathBuf> {
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

#[cfg(unix)]
fn is_executable(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_: &fs::Metadata) -> bool {
    true
}

/// The first symbolic link under `dir` (not following any), relative to it.
fn first_link(dir: &Path, rel: &Path) -> Option<PathBuf> {
    let mut entries: Vec<_> = fs::read_dir(dir.join(rel)).ok()?.flatten().collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = rel.join(entry.file_name());
        let kind = entry.file_type().ok()?;
        if kind.is_symlink() {
            return Some(path);
        }
        if kind.is_dir()
            && let Some(found) = first_link(dir, &path)
        {
            return Some(found);
        }
    }
    None
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
    let Ok(meta) = fs::symlink_metadata(&script) else {
        return Err(format!("its script {run} is missing"));
    };
    if meta.file_type().is_symlink() {
        return Err("its script is a link, so machines will skip this hook".to_owned());
    }
    if !meta.is_file() {
        return Err(format!("its script {run} is missing"));
    }
    // What runs must be exactly what the update shows: no file in the folder may point
    // elsewhere, since a change there wouldn't look like a change to this hook.
    if let Some(link) = first_link(dir, Path::new("")) {
        return Err(format!(
            "its folder holds a link ({}), so machines will skip this hook",
            link.display()
        ));
    }
    if !is_executable(&meta) {
        return Err(format!(
            "its script {run} isn't executable (mark it with chmod +x and commit)"
        ));
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
    let agents = knowledge.join("AGENTS.md");
    if fs::symlink_metadata(&agents).is_ok_and(|m| m.is_file()) {
        d.instructions = Some(agents);
    }
    d.agents = agents_in(&knowledge.join("agents"), &mut d.problems);
    let hooks_dir = knowledge.join("hooks");
    if fs::symlink_metadata(&hooks_dir).is_ok_and(|m| m.file_type().is_symlink()) {
        d.problems
            .push("the hooks folder is a link, so machines will skip every hook".to_owned());
        return d;
    }
    let mut entries: Vec<_> = Vec::new();
    for entry in fs::read_dir(&hooks_dir).into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        match fs::symlink_metadata(entry.path()) {
            Ok(m) if m.file_type().is_symlink() => d.problems.push(format!(
                "hook {name} skipped: its folder is a link, so machines will skip this hook"
            )),
            Ok(m) if m.is_dir() => entries.push(name),
            _ => {}
        }
    }
    entries.sort();
    for name in entries {
        match hook(&hooks_dir.join(&name), &name) {
            Ok(spec) => d.hooks.push(spec),
            Err(e) => d.problems.push(format!("hook {name} skipped: {e}")),
        }
    }
    d
}
