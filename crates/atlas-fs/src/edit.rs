//! Finding, reading and writing the file behind an edit. Writes stay inside the repo
//! (cap-std), replace the file atomically and never go through a symlink.

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};

use atlas_core::catalog::SkillHome;
use atlas_core::catalog::edit::{EditTarget, RepoKind};
use atlas_core::kernel::Scope;
use cap_std::ambient_authority;
use cap_std::fs::Dir;
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
        EditTarget::Memory { file, .. } => {
            // The catalog already found the note's file; accept only a plain path in memory/.
            let plain = Path::new(file)
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
            if !plain
                || !file.starts_with("memory/")
                || Path::new(file).extension().is_none_or(|e| e != "md")
            {
                return None;
            }
            let root = sources.knowledge.clone();
            Located {
                shown: sources.display(&root.join(file)),
                kind: RepoKind::Knowledge,
                root,
                rel: file.clone(),
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
    use crate::library::{LibraryError, write_file};
    write_file(&located.root, &located.rel, text.as_bytes()).map_err(|e| match e {
        LibraryError::Symlink(_) => EditError::Symlink,
        other => EditError::Io(other.to_string()),
    })
}
