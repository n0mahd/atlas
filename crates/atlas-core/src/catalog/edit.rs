//! What can be edited, and the rules a save must follow. Phase 2.

use crate::kernel::{Scope, Slug};

/// An item the owner can edit by hand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EditTarget {
    /// A skill's `SKILL.md`, in the store or a project.
    Skill { home: super::SkillHome, name: Slug },
    /// A memory note in the store. Agents' own per-repo notes aren't in git, so they stay
    /// read-only: every edit must be a commit (strategy: nothing is lost).
    Memory {
        name: String,
        /// The note's file relative to the knowledge repo, as the catalog found it.
        file: String,
    },
    /// An instruction file: the store's `AGENTS.md` or a project's `AGENTS.md`/`CLAUDE.md`.
    Instructions { scope: Scope, file: String },
}

/// Which kind of repo an edit lands in; it decides the commit and push rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RepoKind {
    /// The private knowledge repo.
    Knowledge,
    /// The public skills repo. Never pushed by Atlas: public changes get a privacy check
    /// by hand first.
    PublicSkills,
    /// A project repo. Edits commit to `main` (decision 0003).
    Project(Slug),
}

impl RepoKind {
    /// The branch a save must be on, if any.
    #[must_use]
    pub const fn required_branch(&self) -> Option<&'static str> {
        match self {
            Self::Project(_) => Some("main"),
            Self::Knowledge | Self::PublicSkills => None,
        }
    }

    /// Whether Atlas may push after committing, given the owner's setting. Only project
    /// repos: Knowledge is pushed when an update is sent, public skills never (0022).
    #[must_use]
    pub const fn may_push(&self, push_setting: bool) -> bool {
        push_setting && matches!(self, Self::Project(_))
    }
}

/// The biggest file Atlas will save.
pub const MAX_EDIT_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EditRejected {
    #[error("the file is larger than Atlas saves ({MAX_EDIT_BYTES} bytes)")]
    TooLarge,
    #[error("the frontmatter block (---) is never closed")]
    UnclosedFrontmatter,
    #[error("the file contains a NUL byte")]
    Binary,
}

/// Normalises text from a browser form (CRLF to LF, one trailing newline) and checks it
/// can be saved.
///
/// # Errors
/// Too large, binary, or a markdown file whose frontmatter would no longer parse.
pub fn prepare(text: &str, markdown: bool) -> Result<String, EditRejected> {
    let mut out = text.replace("\r\n", "\n").replace('\r', "\n");
    if out.contains('\0') {
        return Err(EditRejected::Binary);
    }
    while out.ends_with("\n\n") {
        out.pop();
    }
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    if out.len() > MAX_EDIT_BYTES {
        return Err(EditRejected::TooLarge);
    }
    if markdown && super::split_frontmatter(&out).is_err() {
        return Err(EditRejected::UnclosedFrontmatter);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_browser_text() {
        assert_eq!(prepare("a\r\nb", true).unwrap(), "a\nb\n");
        assert_eq!(prepare("a\n\n\n", false).unwrap(), "a\n");
        assert_eq!(prepare("", false).unwrap(), "");
    }

    #[test]
    fn rejects_what_would_break_the_store() {
        assert_eq!(
            prepare("---\nname: x\n", true),
            Err(EditRejected::UnclosedFrontmatter)
        );
        assert!(prepare("---\nname: x\n", false).is_ok());
        assert_eq!(prepare("a\0b", false), Err(EditRejected::Binary));
        assert_eq!(
            prepare(&"x".repeat(MAX_EDIT_BYTES + 1), false),
            Err(EditRejected::TooLarge)
        );
    }

    #[test]
    fn push_rules() {
        let project = RepoKind::Project(Slug::new("shop").unwrap());
        assert_eq!(project.required_branch(), Some("main"));
        assert!(!RepoKind::PublicSkills.may_push(true));
        assert!(!RepoKind::Knowledge.may_push(true));
        assert!(project.may_push(true));
        assert!(!project.may_push(false));
    }
}
