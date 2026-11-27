//! A harness's global instruction file: a link to the store's `AGENTS.md`, or, for
//! Claude Code's `CLAUDE.md`, an `@<path>` import line. Atlas never overwrites a file
//! with content of its own.

use std::path::{Path, PathBuf};

use crate::skills::normalize;

/// What sits at the harness's instruction file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionFile {
    Missing,
    /// A symlink, with its target as read (relative targets resolve against the
    /// file's folder).
    Link(PathBuf),
    /// A regular file and its contents.
    File(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionAction {
    /// Already right.
    Keep,
    CreateLink {
        target: PathBuf,
    },
    /// The link points at an older Atlas path.
    ReplaceLink {
        from: PathBuf,
        to: PathBuf,
    },
    /// Claude Code only: write these contents (the file with the import appended).
    Write(Vec<u8>),
    Conflict(InstructionConflict),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionConflict {
    /// A regular file with the user's own content is in the way.
    UserFile,
    /// A link points somewhere Atlas doesn't manage.
    ForeignLink { target: PathBuf },
    /// Claude Code would read the import as several words.
    UnimportablePath,
}

/// For harnesses that read `AGENTS.md` directly (Codex, `OpenCode`, Pi, Antigravity):
/// the instruction file should be a link to `store`.
///
/// `file` is the instruction file's own path, used to resolve relative link targets.
/// A link into one of `managed_roots` is Atlas's and may be repointed.
#[must_use]
pub fn plan_instruction_link(
    file: &Path,
    current: &InstructionFile,
    store: &Path,
    managed_roots: &[PathBuf],
) -> InstructionAction {
    let store = normalize(store);
    match current {
        InstructionFile::Missing => InstructionAction::CreateLink { target: store },
        InstructionFile::File(_) => InstructionAction::Conflict(InstructionConflict::UserFile),
        InstructionFile::Link(t) => {
            let target = resolve(file, t);
            if target == store {
                InstructionAction::Keep
            } else if managed_roots
                .iter()
                .any(|r| target.starts_with(normalize(r)))
            {
                InstructionAction::ReplaceLink {
                    from: target,
                    to: store,
                }
            } else {
                InstructionAction::Conflict(InstructionConflict::ForeignLink { target })
            }
        }
    }
}

/// The import line Claude Code reads for `store`.
#[must_use]
pub fn claude_import(store: &Path) -> String {
    format!("@{}", store.display())
}

/// For Claude Code's `~/.claude/CLAUDE.md`: the file should hold an `@<store>` import
/// line. A missing file is created with just that line; an existing one gets the line
/// appended if absent, and nothing else in it changes. A link is left alone unless it
/// already points at `store` (Claude Code then reads the store directly), because
/// appending would write through it.
#[must_use]
pub fn plan_claude_md(file: &Path, current: &InstructionFile, store: &Path) -> InstructionAction {
    let store = normalize(store);
    let import = claude_import(&store);
    if import.chars().any(char::is_whitespace) {
        return InstructionAction::Conflict(InstructionConflict::UnimportablePath);
    }
    match current {
        InstructionFile::Missing => InstructionAction::Write(format!("{import}\n").into_bytes()),
        InstructionFile::Link(t) => {
            let target = resolve(file, t);
            if target == store {
                InstructionAction::Keep
            } else {
                InstructionAction::Conflict(InstructionConflict::ForeignLink { target })
            }
        }
        InstructionFile::File(text) => {
            if text.lines().any(|l| l.trim() == import) {
                return InstructionAction::Keep;
            }
            let mut out = text.clone();
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&import);
            out.push('\n');
            InstructionAction::Write(out.into_bytes())
        }
    }
}

fn resolve(file: &Path, target: &Path) -> PathBuf {
    normalize(&file.parent().unwrap_or(Path::new("/")).join(target))
}
