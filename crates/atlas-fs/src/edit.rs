//! Finding, reading and writing the file behind an edit. Writes stay inside the repo
//! (cap-std), replace the file atomically and never go through a symlink.

use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};

use atlas_core::catalog::SkillHome;
use atlas_core::catalog::edit::{EditTarget, RepoKind};
use atlas_core::kernel::Scope;
use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use sha2::{Digest, Sha256};

use crate::paths::Sources;

/// Where an edit lands.
#[derive(Clone, Debug)]
pub struct Located {
    pub kind: RepoKind,
    /// The repo's top folder.
    pub root: PathBuf,
    /// The file, relative to `root`.
    pub rel: String,
    /// The file as shown to the owner (home as `~`).
    pub shown: String,
    pub markdown: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("there's no such item to edit")]
    NotFound,
    #[error("this file is a link; edit it where it lives")]
    Symlink,
    #[error("the file changed since you opened it")]
    Changed,
    #[error("{0}")]
    Io(String),
}

impl From<io::Error> for EditError {
    fn from(e: io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// A short fingerprint of a file's text, to notice edits made elsewhere meanwhile.
#[must_use]
pub fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest
        .iter()
        .take(16)
        .fold(String::with_capacity(32), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// Finds the repo and file for an edit target, if it exists.
#[must_use]
pub fn locate(sources: &Sources, target: &EditTarget) -> Option<Located> {
    let located = match target {
        EditTarget::Skill { home, name } => {
            let (kind, root, rel) = match home {
                SkillHome::Private => (
                    RepoKind::Knowledge,
                    sources.knowledge.clone(),
                    format!("skills/{name}/SKILL.md"),
                ),
                SkillHome::Public => (
                    RepoKind::PublicSkills,
                    sources.public_skills.clone()?,
                    format!("{name}/SKILL.md"),
                ),
                SkillHome::Project(p) => {
                    let project = sources.projects.iter().find(|x| &x.name == p)?;
                    (
                        RepoKind::Project(p.clone()),
                        project.root.clone(),
                        format!(".agents/skills/{name}/SKILL.md"),
                    )
                }
            };
            Located {
                shown: sources.display(&root.join(&rel)),
                kind,
                root,
                rel,
                markdown: true,
            }
        }
        EditTarget::Memory { name } => {
            // Notes are found by their name, which may differ from the file name.
            let rel = find_memory(&sources.knowledge, name)?;
            let root = sources.knowledge.clone();
            Located {
                shown: sources.display(&root.join(&rel)),
                kind: RepoKind::Knowledge,
                root,
                rel,
                markdown: true,
            }
        }
        EditTarget::Instructions { scope, file } => {
            if !matches!(file.as_str(), "AGENTS.md" | "CLAUDE.md") {
                return None;
            }
            let (kind, root) = match scope {
                Scope::Global if file == "AGENTS.md" => {
                    (RepoKind::Knowledge, sources.knowledge.clone())
                }
                Scope::Global => return None,
                Scope::Project(p) => {
                    let project = sources.projects.iter().find(|x| &x.name == p)?;
                    (RepoKind::Project(p.clone()), project.root.clone())
                }
            };
            Located {
                shown: sources.display(&root.join(file)),
                kind,
                root,
                rel: file.clone(),
                markdown: true,
            }
        }
    };
    located.root.join(&located.rel).exists().then_some(located)
}

/// The path of the memory note named `name`, relative to the knowledge repo.
fn find_memory(knowledge: &Path, name: &str) -> Option<String> {
    let catalog_name_matches = |path: &Path, stem: &str| -> bool {
        let Ok(text) = std::fs::read_to_string(path) else {
            return stem == name;
        };
        let declared = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split("\n---").next())
            .and_then(|fm| fm.lines().find_map(|l| l.strip_prefix("name:")))
            .map(|n| n.trim().trim_matches(['"', '\'']).to_owned());
        declared.map_or(stem == name, |d| d == name)
    };
    let memory = knowledge.join("memory");
    let mut stack = vec![memory.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
                stack.push(path);
            } else if ft.is_file()
                && let Some(stem) = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".md"))
                && stem != "index"
                && catalog_name_matches(&path, stem)
            {
                return path
                    .strip_prefix(knowledge)
                    .ok()
                    .map(|p| p.to_string_lossy().into_owned());
            }
        }
    }
    None
}

fn open_root(located: &Located) -> Result<Dir, EditError> {
    Ok(Dir::open_ambient_dir(&located.root, ambient_authority())?)
}

/// Reads the file's full text, refusing links.
///
/// # Errors
/// Missing, a symlink, too big or unreadable.
pub fn read_raw(located: &Located) -> Result<String, EditError> {
    let dir = open_root(located)?;
    let meta = dir
        .symlink_metadata(&located.rel)
        .map_err(|_| EditError::NotFound)?;
    if meta.file_type().is_symlink() {
        return Err(EditError::Symlink);
    }
    let mut text = String::new();
    dir.open(&located.rel)?
        .take(atlas_core::catalog::edit::MAX_EDIT_BYTES as u64 + 1)
        .read_to_string(&mut text)?;
    Ok(text)
}

/// Replaces the file with `text` if it still matches `expected` (a [`content_hash`]).
/// Returns the text it replaced, so a failed commit can put it back.
///
/// # Errors
/// The file changed meanwhile, is a link, or can't be written.
pub fn write_if_unchanged(
    located: &Located,
    expected: &str,
    text: &str,
) -> Result<String, EditError> {
    let before = read_raw(located)?;
    if content_hash(&before) != expected {
        return Err(EditError::Changed);
    }
    write_raw(located, text)?;
    Ok(before)
}

/// Writes the file atomically: a temporary file beside it, then a rename.
///
/// # Errors
/// A link, or the write fails.
pub fn write_raw(located: &Located, text: &str) -> Result<(), EditError> {
    let dir = open_root(located)?;
    if dir
        .symlink_metadata(&located.rel)
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        return Err(EditError::Symlink);
    }
    let rel = Path::new(&located.rel);
    let tmp = rel.with_file_name(format!(
        ".{}.atlas-tmp",
        rel.file_name()
            .map_or_else(|| "file".into(), |n| n.to_string_lossy())
    ));
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    let mut file = dir.open_with(&tmp, &options)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()?;
    drop(file);
    dir.rename(&tmp, &dir, rel)?;
    Ok(())
}
