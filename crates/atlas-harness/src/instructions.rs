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

/// For Claude Code's `~/.claude/CLAUDE.md`: the file should hold one `@<store>` import
/// line. A missing file is created with just that line. In an existing file, an import
/// that points into one of `managed_roots` (an older Atlas path, or a folder Atlas has
/// adopted from earlier scripts, such as `@~/Knowledge/AGENTS.md`) is replaced in place
/// rather than duplicated, so Claude Code doesn't load the instructions twice; otherwise
/// the line is appended. Nothing else changes. A link is left alone unless it already
/// points at `store`, because writing would go through it. `home` expands `@~/` imports.
#[must_use]
pub fn plan_claude_md(
    file: &Path,
    current: &InstructionFile,
    store: &Path,
    managed_roots: &[PathBuf],
    home: &Path,
) -> InstructionAction {
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
            let imported = |line: &str| -> Option<PathBuf> {
                let path = line.trim().strip_prefix('@')?;
                let path = match path.strip_prefix("~/") {
                    Some(rest) => home.join(rest),
                    None => PathBuf::from(path),
                };
                Some(if path.is_absolute() {
                    normalize(&path)
                } else {
                    resolve(file, &path)
                })
            };
            let managed = |p: &Path| managed_roots.iter().any(|r| p.starts_with(normalize(r)));
            let mut placed = false;
            let mut lines: Vec<String> = Vec::new();
            for line in text.lines() {
                match imported(line) {
                    Some(p) if p == store || managed(&p) => {
                        // One Atlas import, where the first one was: a line already
                        // importing the store stays as written; an older managed path
                        // becomes the store's import; repeats go.
                        if !placed {
                            lines.push(if p == store {
                                line.to_owned()
                            } else {
                                import.clone()
                            });
                            placed = true;
                        }
                    }
                    _ => lines.push(line.to_owned()),
                }
            }
            if !placed {
                lines.push(import.clone());
            }
            let mut out = lines.join("\n");
            out.push('\n');
            if out == *text || (!text.ends_with('\n') && out.trim_end_matches('\n') == text) {
                return InstructionAction::Keep;
            }
            InstructionAction::Write(out.into_bytes())
        }
    }
}

fn resolve(file: &Path, target: &Path) -> PathBuf {
    normalize(&file.parent().unwrap_or(Path::new("/")).join(target))
}
