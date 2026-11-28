//! The one way a page changes more than a single edited file: switching a skill for an
//! agent, creating, moving or deleting a skill, importing something into Atlas, merging
//! instruction files. Every such change follows the save rules of decision 0016:
//! a passkey session and its CSRF token, one change at a time under the git lock, project
//! repos only on `main`, no path the change touches may have uncommitted edits made
//! outside Atlas, and one commit per repo of exactly those paths. If writing or a commit
//! fails, every touched path is put back as HEAD has it, so nothing is half-done.
//!
//! A multi-repo change (a skill moved from a project to Global) commits the repos in the
//! order given; callers list the repo that gains files first, so a failure part way
//! leaves an extra copy rather than a lost one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use atlas_core::catalog::edit::RepoKind;
use axum::Json;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Redirect, Response};
use serde::Serialize;

use crate::AppState;
use crate::edit::{forbidden, wants_json, writer};

/// One repo's part of a change.
#[derive(Clone, Debug)]
pub(crate) struct RepoChange {
    pub kind: RepoKind,
    /// The repo's top folder.
    pub root: PathBuf,
    /// Files or folders, relative to `root`, that the change writes or removes.
    pub paths: Vec<PathBuf>,
    /// The commit message, such as "Turn pdf-tools off for Codex in Atlas".
    pub message: String,
}

/// Writes the change's files. Runs on a blocking thread, under the git lock, after the
/// checks pass. Return a sentence for the owner on failure.
pub(crate) type Apply = Box<dyn FnOnce() -> Result<(), String> + Send>;

#[derive(Debug)]
pub(crate) enum Outcome {
    /// At least one repo got a commit.
    Committed { kinds: Vec<RepoKind> },
    /// The change wrote nothing new.
    Unchanged,
}

#[derive(Debug)]
pub(crate) enum ChangeError {
    /// No session, a stale CSRF token or a cross-origin request.
    Forbidden,
    /// A rule refused it or it failed; the sentence says why. Nothing changed.
    Refused(String),
}

impl ChangeError {
    /// The response for a refused change: 403 for a session problem, else 409 with text.
    pub(crate) fn into_response(self) -> Response {
        match self {
            Self::Forbidden => forbidden("Your session ended. Sign in again; nothing changed.\n"),
            Self::Refused(msg) => (axum::http::StatusCode::CONFLICT, msg).into_response(),
        }
    }
}

fn shown(root: &Path) -> String {
    root.file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
}

/// Checks the session, then runs `apply` and commits `repos`, following the rules above.
pub(crate) async fn commit_change(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    repos: Vec<RepoChange>,
    apply: Apply,
) -> Result<Outcome, ChangeError> {
    let w = Arc::clone(writer(s).map_err(|_| ChangeError::Forbidden)?);
    if !w.authorised(headers, csrf) {
        return Err(ChangeError::Forbidden);
    }
    let writer = Arc::clone(&w);
    let outcome = tokio::task::spawn_blocking(move || run(&writer, &repos, apply))
        .await
        .map_err(|_| {
            ChangeError::Refused("The change stopped unexpectedly; nothing changed.".to_owned())
        })?;
    // Whatever happened, the next page should read the store as it now is.
    s.invalidate().await;
    outcome
}

fn run(w: &crate::Writer, repos: &[RepoChange], apply: Apply) -> Result<Outcome, ChangeError> {
    let refuse = |m: String| ChangeError::Refused(m);
    let _guard = w
        .git
        .lock()
        .map_err(|_| refuse("Another change is stuck.".to_owned()))?;
    let mut opened = Vec::with_capacity(repos.len());
    for change in repos {
        let repo = atlas_git::Repo::open(&change.root).map_err(|e| refuse(e.to_string()))?;
        if let Some(branch) = change.kind.required_branch() {
            let actual = repo.current_branch().map_err(|e| refuse(e.to_string()))?;
            if actual.as_deref() != Some(branch) {
                return Err(refuse(format!(
                    "{} is on {}, not {branch}. Changes commit to {branch}; switch branch first.",
                    shown(&change.root),
                    actual.unwrap_or_else(|| "no branch".to_owned())
                )));
            }
        }
        for rel in &change.paths {
            let clean = repo.is_clean(rel).map_err(|e| refuse(e.to_string()))?;
            if !clean {
                return Err(refuse(format!(
                    "{} has uncommitted changes made outside Atlas. Commit or discard them first; nothing changed.",
                    Path::new(&shown(&change.root)).join(rel).display()
                )));
            }
        }
        opened.push(repo);
    }
    let restore_all = |opened: &[atlas_git::Repo]| {
        for (repo, change) in opened.iter().zip(repos) {
            let paths: Vec<&Path> = change.paths.iter().map(PathBuf::as_path).collect();
            if let Err(e) = repo.restore_paths(&paths) {
                tracing::error!(
                    "restoring {} after a failed change: {e}",
                    shown(&change.root)
                );
            }
        }
    };
    if let Err(msg) = apply() {
        restore_all(&opened);
        return Err(refuse(format!("{msg} Nothing changed.")));
    }
    let mut kinds = Vec::new();
    for (i, (repo, change)) in opened.iter().zip(repos).enumerate() {
        let paths: Vec<&Path> = change.paths.iter().map(PathBuf::as_path).collect();
        match repo.commit_paths(&paths, &change.message, change.kind.required_branch()) {
            Ok(_) => kinds.push(change.kind.clone()),
            Err(atlas_git::GitError::NothingToCommit) => {}
            Err(e) => {
                // Repos before this one are committed (they gained files first); put this
                // one and any later ones back.
                restore_all(&opened[i..]);
                let text = e.to_string();
                let msg = if text.contains("tell me who you are") || text.contains("empty ident") {
                    format!(
                        "{} has no git name and email set, so git won't commit. Set user.name and user.email in it, then try again.",
                        shown(&change.root)
                    )
                } else {
                    format!("The commit in {} failed: {text}", shown(&change.root))
                };
                return Err(refuse(msg));
            }
        }
    }
    if kinds.is_empty() {
        return Ok(Outcome::Unchanged);
    }
    for (repo, change) in opened.iter().zip(repos) {
        if kinds.contains(&change.kind)
            && change.kind.may_push(w.settings().push_after_save)
            && let Err(e) = repo.push()
        {
            tracing::warn!("push after change failed: {e}");
        }
    }
    Ok(Outcome::Committed { kinds })
}

#[derive(Serialize)]
struct Done {
    next: String,
    send: bool,
}

/// The answer to a change: JSON `{next, send}` for the page script (which then asks for
/// the passkey tap that sends it), or a redirect without script. `next` gets `changed=1`
/// or `unchanged=1`.
pub(crate) fn respond(
    s: &AppState,
    headers: &HeaderMap,
    outcome: &Outcome,
    next: &str,
) -> Response {
    let sep = if next.contains('?') { '&' } else { '?' };
    let (next, knowledge) = match outcome {
        Outcome::Committed { kinds } => (
            format!("{next}{sep}changed=1"),
            kinds.contains(&RepoKind::Knowledge),
        ),
        Outcome::Unchanged => (format!("{next}{sep}unchanged=1"), false),
    };
    let send = knowledge && s.writer.as_ref().is_some_and(|w| w.settings().send_on_save);
    if wants_json(headers) {
        Json(Done { next, send }).into_response()
    } else {
        Redirect::to(&next).into_response()
    }
}
