use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use atlas_core::catalog::{
    Catalog, HookDef, HookManifest, InstructionFile, MemoryFrontmatter, MemoryPage, Problem, Skill,
    SkillFrontmatter, SkillHome, split_frontmatter,
};
use atlas_core::kernel::{HarnessKind, Scope, Slug};
use atlas_core::untrusted::{Markdown, Untrusted};
use cap_std::ambient_authority;
use cap_std::fs::Dir;
use serde::de::DeserializeOwned;

use crate::paths::Sources;

/// Files bigger than this are skipped with a problem instead of read.
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// A skill folder listing stops after this many files.
const MAX_LISTED_FILES: usize = 200;
/// Memory pages read per snapshot; more are reported, not silently dropped.
const MAX_MEMORY_PAGES: usize = 5000;
/// Skills read from one home.
const MAX_SKILLS_PER_HOME: usize = 2000;
/// Bytes read for one snapshot. A huge or hostile repo can't exhaust memory.
const MAX_SNAPSHOT_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DEPTH: usize = 4;

/// Reads everything in the store. Never fails as a whole: anything unreadable becomes a
/// [`Problem`] next to the items that did load.
#[must_use]
pub fn load_catalog(sources: &Sources) -> Catalog {
    let mut r = Reader {
        sources,
        catalog: Catalog::default(),
        bytes: 0,
    };

    if let Some(knowledge) = r.open_root(&sources.knowledge) {
        r.load_instruction(&knowledge, &sources.knowledge, "AGENTS.md", &Scope::Global);
        r.load_skills(
            &knowledge,
            &sources.knowledge,
            "skills",
            &SkillHome::Private,
        );
        r.load_memory(
            &knowledge,
            &sources.knowledge,
            "memory",
            &Scope::Global,
            None,
        );
        r.load_hooks(&knowledge, &sources.knowledge, "hooks", &Scope::Global);
    }
    if let Some(public) = &sources.public_skills
        && let Some(dir) = r.open_root(public)
    {
        r.load_skills(&dir, public, ".", &SkillHome::Public);
    }
    for project in &sources.projects {
        // Claude Code keeps its own notes per repo, outside the repo.
        let claude_memory = atlas_harness::claude_project_memory(&sources.home, &project.root);
        if let Ok(dir) = Dir::open_ambient_dir(&claude_memory, ambient_authority()) {
            let scope = Scope::Project(project.name.clone());
            r.load_memory(
                &dir,
                &claude_memory,
                ".",
                &scope,
                Some(HarnessKind::ClaudeCode),
            );
        }
        let Some(dir) = r.open_root(&project.root) else {
            continue;
        };
        let scope = Scope::Project(project.name.clone());
        for file in ["AGENTS.md", "CLAUDE.md"] {
            r.load_instruction(&dir, &project.root, file, &scope);
        }
        let home = SkillHome::Project(project.name.clone());
        r.load_skills(&dir, &project.root, ".agents/skills", &home);
        r.load_hooks(&dir, &project.root, ".agents/hooks", &scope);
    }

    r.catalog.sort();
    r.catalog
}

struct Reader<'a> {
    sources: &'a Sources,
    catalog: Catalog,
    bytes: u64,
}

impl Reader<'_> {
    fn problem(&mut self, path: &Path, message: impl Into<String>) {
        self.catalog.problems.push(Problem {
            path: self.sources.display(path),
            message: message.into(),
        });
    }

    fn open_root(&mut self, root: &Path) -> Option<Dir> {
        match Dir::open_ambient_dir(root, ambient_authority()) {
            Ok(dir) => Some(dir),
            Err(e) => {
                self.problem(root, format!("can't open: {e}"));
                None
            }
        }
    }

    /// Opens a folder inside `dir`. A missing folder is normal and returns None quietly.
    fn open_sub(&mut self, dir: &Dir, root: &Path, rel: &str) -> Option<Dir> {
        if rel == "." {
            return dir.try_clone().ok();
        }
        match dir.open_dir(rel) {
            Ok(d) => Some(d),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => {
                self.problem(&root.join(rel), format!("can't open: {}", describe(&e)));
                None
            }
        }
    }

    /// Reads a text file at `rel` inside `dir`, following links only while they stay
    /// inside `dir`. Missing files return None quietly; other failures are reported.
    fn read_text(&mut self, dir: &Dir, shown: &Path, rel: &str) -> Option<String> {
        match self.try_read(dir, shown, rel) {
            Read::Text(text) => Some(text),
            Read::Missing | Read::Failed => None,
        }
    }

    fn try_read(&mut self, dir: &Dir, shown: &Path, rel: &str) -> Read {
        match dir.metadata(rel) {
            Ok(m) if m.len() > MAX_FILE_BYTES => {
                self.problem(
                    shown,
                    format!("skipped: larger than {MAX_FILE_BYTES} bytes"),
                );
                return Read::Failed;
            }
            Ok(m) if !m.is_file() => {
                self.problem(shown, "skipped: not a regular file");
                return Read::Failed;
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound && dir.symlink_metadata(rel).is_err() => {
                return Read::Missing;
            }
            Err(e) => {
                self.problem(shown, format!("can't read: {}", describe(&e)));
                return Read::Failed;
            }
        }
        if let Some(reason) = hidden_link_target(dir, rel) {
            self.problem(shown, reason);
            return Read::Failed;
        }
        if self.bytes > MAX_SNAPSHOT_BYTES {
            self.problem(shown, "skipped: the store is over Atlas's read budget");
            return Read::Failed;
        }
        // Read at most the limit, even if the file grew since the size check.
        let mut text = String::new();
        let read = dir
            .open(rel)
            .and_then(|f| f.take(MAX_FILE_BYTES + 1).read_to_string(&mut text));
        match read {
            Ok(n) if n as u64 > MAX_FILE_BYTES => {
                self.problem(
                    shown,
                    format!("skipped: larger than {MAX_FILE_BYTES} bytes"),
                );
                Read::Failed
            }
            Ok(n) => {
                self.bytes += n as u64;
                Read::Text(text)
            }
            Err(e) => {
                self.problem(shown, format!("can't read: {}", describe(&e)));
                Read::Failed
            }
        }
    }

    /// Splits off and parses frontmatter. A bad block is reported and treated as empty.
    fn parse_front<T: DeserializeOwned + Default>(
        &mut self,
        shown: &Path,
        text: &str,
    ) -> (T, String) {
        match split_frontmatter(text) {
            Ok((None, body)) => (T::default(), body.to_owned()),
            Ok((Some(fm), body)) if fm.0.trim().is_empty() => (T::default(), body.to_owned()),
            Ok((Some(fm), body)) => match serde_yaml_ng::from_str::<T>(fm.0) {
                Ok(parsed) => (parsed, body.to_owned()),
                // Harnesses accept `description: text: with colons` unquoted; so does Atlas.
                Err(e) => {
                    if let Ok(parsed) = serde_yaml_ng::from_str::<T>(&quote_plain_values(fm.0)) {
                        (parsed, body.to_owned())
                    } else {
                        self.problem(shown, format!("frontmatter isn't valid YAML: {e}"));
                        (T::default(), body.to_owned())
                    }
                }
            },
            Err(e) => {
                self.problem(shown, e.to_string());
                (T::default(), text.to_owned())
            }
        }
    }

    fn load_instruction(&mut self, dir: &Dir, root: &Path, file: &str, scope: &Scope) {
        let shown = root.join(file);
        if let Some(text) = self.read_text(dir, &shown, file) {
            self.catalog.instructions.push(InstructionFile {
                scope: scope.clone(),
                file_name: file.to_owned(),
                path: self.sources.display(&shown),
                body: Untrusted::new(Markdown(text)),
            });
        }
    }

    fn load_skills(&mut self, dir: &Dir, root: &Path, rel: &str, home: &SkillHome) {
        let Some(skills) = self.open_sub(dir, root, rel) else {
            return;
        };
        let mut count = 0;
        let base = if rel == "." {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        for (name, kind) in self.list(&skills, &base) {
            let shown = base.join(&name);
            match kind {
                EntryKind::Symlink => {
                    self.problem(
                        &shown,
                        "is a symlink; Atlas doesn't follow links in the store",
                    );
                    continue;
                }
                EntryKind::File => continue,
                EntryKind::Dir => {}
            }
            count += 1;
            if count > MAX_SKILLS_PER_HOME {
                self.problem(
                    &base,
                    format!("only the first {MAX_SKILLS_PER_HOME} skills are read"),
                );
                break;
            }
            let Ok(slug) = Slug::new(name.clone()) else {
                self.problem(
                    &shown,
                    "skill folder name must be lowercase a-z, 0-9 and hyphens",
                );
                continue;
            };
            let Some(folder) = self.open_sub(&skills, &base, &name) else {
                continue;
            };
            let skill_md = shown.join("SKILL.md");
            let text = match self.try_read(dir, &skill_md, &join(rel, &format!("{name}/SKILL.md")))
            {
                Read::Text(text) => text,
                Read::Missing if rel != "." => {
                    self.problem(&shown, "folder has no SKILL.md");
                    continue;
                }
                Read::Missing | Read::Failed => continue,
            };
            let (fm, body): (SkillFrontmatter, String) = self.parse_front(&skill_md, &text);
            let declared_name = fm.name.filter(|n| n != slug.as_str());
            let mut files = Vec::new();
            self.walk(&folder, &shown, "", 0, MAX_LISTED_FILES, &mut files);
            files.retain(|f| f != "SKILL.md");
            self.catalog.skills.push(Skill {
                name: slug,
                home: home.clone(),
                description: fm.description.trim().to_owned(),
                declared_name,
                path: self.sources.display(&skill_md),
                files,
                body: Untrusted::new(Markdown(body)),
            });
        }
    }

    fn load_memory(
        &mut self,
        root_dir: &Dir,
        root: &Path,
        rel: &str,
        scope: &Scope,
        kept_by: Option<HarnessKind>,
    ) {
        let Some(memory) = self.open_sub(root_dir, root, rel) else {
            return;
        };
        let base = if rel == "." {
            root.to_path_buf()
        } else {
            root.join(rel)
        };
        let mut files = Vec::new();
        self.walk(&memory, &base, "", 0, MAX_MEMORY_PAGES, &mut files);
        if files.len() >= MAX_MEMORY_PAGES {
            self.problem(
                &base,
                format!("only the first {MAX_MEMORY_PAGES} memory files are read"),
            );
        }
        for rel in files {
            let Some(stem) = rel.strip_suffix(".md") else {
                continue;
            };
            // Index pages are maps of the other pages, not facts.
            let file = rel.rsplit('/').next().unwrap_or(&rel);
            if file == "index.md" || file == "MEMORY.md" {
                continue;
            }
            let shown = base.join(&rel);
            let in_repo = join(rel_dir(rel_base(&base, root)), &rel);
            let Some(text) = self.read_text(root_dir, &shown, &in_repo) else {
                continue;
            };
            let (fm, body): (MemoryFrontmatter, String) = self.parse_front(&shown, &text);
            // Pages link by their frontmatter name, which may differ from the file name
            // (host pages are often prefixed with the host).
            let name = fm
                .name
                .clone()
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| stem.rsplit('/').next().unwrap_or(stem).to_owned());
            let mut page = MemoryPage::new(name, self.sources.display(&shown), fm, &body);
            page.scope = scope.clone();
            page.kept_by = kept_by;
            page.file = in_repo;
            self.catalog.memory.push(page);
        }
    }

    fn load_hooks(&mut self, dir: &Dir, root: &Path, rel: &str, scope: &Scope) {
        let Some(hooks) = self.open_sub(dir, root, rel) else {
            return;
        };
        let base = root.join(rel);
        for (name, kind) in self.list(&hooks, &base) {
            let shown = base.join(&name);
            if kind != EntryKind::Dir {
                if kind == EntryKind::Symlink {
                    self.problem(
                        &shown,
                        "is a symlink; Atlas doesn't follow links in the store",
                    );
                }
                continue;
            }
            let Ok(slug) = Slug::new(name.clone()) else {
                self.problem(
                    &shown,
                    "hook folder name must be lowercase a-z, 0-9 and hyphens",
                );
                continue;
            };
            let Some(folder) = self.open_sub(&hooks, &base, &name) else {
                continue;
            };
            let manifest_path = shown.join("hook.toml");
            let text = match self.try_read(
                dir,
                &manifest_path,
                &join(rel, &format!("{name}/hook.toml")),
            ) {
                Read::Text(text) => text,
                Read::Missing => {
                    self.problem(&shown, "folder has no hook.toml");
                    continue;
                }
                Read::Failed => continue,
            };
            let manifest = match toml::from_str::<HookManifest>(&text) {
                Ok(m) => m,
                Err(e) => {
                    self.problem(&manifest_path, format!("hook.toml isn't valid: {e}"));
                    continue;
                }
            };
            let plain_name = !manifest.run.is_empty()
                && !manifest.run.contains(['/', '\\'])
                && !manifest.run.starts_with('.');
            if !plain_name {
                self.problem(
                    &manifest_path,
                    "`run` must name a file in the hook's own folder",
                );
            }
            let script_present =
                plain_name && folder.metadata(&manifest.run).is_ok_and(|m| m.is_file());
            if plain_name && !script_present {
                self.problem(
                    &manifest_path,
                    format!("script {:?} is missing", manifest.run),
                );
            }
            self.catalog.hooks.push(HookDef {
                name: slug,
                scope: scope.clone(),
                manifest,
                path: self.sources.display(&manifest_path),
                script_present,
            });
        }
    }

    /// Lists a folder's visible entries, sorted, without following symlinks.
    fn list(&mut self, dir: &Dir, shown: &Path) -> Vec<(String, EntryKind)> {
        let entries = match dir.entries() {
            Ok(e) => e,
            Err(e) => {
                self.problem(shown, format!("can't list: {}", describe(&e)));
                return Vec::new();
            }
        };
        let mut out: Vec<(String, EntryKind)> = entries
            .flatten()
            .filter_map(|entry| {
                let name = entry.file_name().into_string().ok()?;
                if name.starts_with('.') {
                    return None;
                }
                let ft = entry.file_type().ok()?;
                let kind = if ft.is_symlink() {
                    EntryKind::Symlink
                } else if ft.is_dir() {
                    EntryKind::Dir
                } else {
                    EntryKind::File
                };
                Some((name, kind))
            })
            .collect();
        out.sort();
        out
    }

    /// Collects regular files under `dir`, as paths relative to where the walk began.
    fn walk(
        &mut self,
        dir: &Dir,
        shown: &Path,
        prefix: &str,
        depth: usize,
        limit: usize,
        out: &mut Vec<String>,
    ) {
        for (name, kind) in self.list(dir, shown) {
            if out.len() >= limit {
                return;
            }
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            match kind {
                EntryKind::File => out.push(rel),
                EntryKind::Dir if depth + 1 < MAX_DEPTH => {
                    let sub_shown: PathBuf = shown.join(&name);
                    if let Some(sub) = self.open_sub(dir, shown, &name) {
                        self.walk(&sub, &sub_shown, &rel, depth + 1, limit, out);
                    }
                }
                EntryKind::Dir | EntryKind::Symlink => {}
            }
        }
    }
}

/// A link inside a repo may point at another tracked file (a skill kept in `docs/`),
/// but not at hidden files such as `.env` or anything under `.git`.
fn hidden_link_target(dir: &Dir, rel: &str) -> Option<String> {
    let is_link = dir
        .symlink_metadata(rel)
        .is_ok_and(|m| m.file_type().is_symlink());
    if !is_link {
        return None;
    }
    let target = dir.read_link(rel).ok()?;
    let hidden = target.components().any(|c| {
        let part = c.as_os_str().to_string_lossy();
        part.starts_with('.') && part != "." && part != ".."
    });
    hidden.then(|| format!("skipped: links to a hidden file ({})", target.display()))
}

enum Read {
    Text(String),
    Missing,
    Failed,
}

fn join(base: &str, rest: &str) -> String {
    if base == "." {
        rest.to_owned()
    } else {
        format!("{base}/{rest}")
    }
}

/// Quotes top-level `key: value` lines whose value YAML would reject as plain text,
/// such as one with `: ` inside it.
fn quote_plain_values(yaml: &str) -> String {
    yaml.lines()
        .map(|line| {
            let Some((key, value)) = line.split_once(": ") else {
                return line.to_owned();
            };
            let value = value.trim();
            let plain_key = !key.is_empty() && !key.starts_with([' ', '\t', '-', '#']);
            let already =
                value.is_empty() || value.starts_with(['"', '\'', '[', '{', '|', '>', '&', '*']);
            if plain_key && !already {
                format!(
                    "{key}: \"{}\"",
                    value.replace('\\', "\\\\").replace('"', "\\\"")
                )
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum EntryKind {
    Dir,
    File,
    Symlink,
}

fn describe(e: &io::Error) -> String {
    let text = e.to_string();
    if text.contains("outside of the filesystem") || e.kind() == io::ErrorKind::PermissionDenied {
        "it points outside the repo or isn't allowed".to_owned()
    } else {
        text
    }
}

/// The memory folder's path relative to the root it was opened from, or `.`.
fn rel_base<'a>(base: &'a Path, root: &Path) -> &'a Path {
    base.strip_prefix(root).unwrap_or(base)
}

fn rel_dir(p: &Path) -> &str {
    match p.to_str() {
        Some("") | None => ".",
        Some(s) => s,
    }
}
