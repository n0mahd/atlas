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
//! leaves an extra copy rather than a lost one, and the owner is told which repo kept it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use atlas_core::catalog::edit::RepoKind;
use atlas_core::catalog::{PLACEMENTS_FILE, Placements};
use axum::Json;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::Serialize;

use crate::auth::safe_next;
use crate::edit::{forbidden, wants_json};
use crate::{AppError, AppState};

/// One repo's part of a change.
#[derive(Clone, Debug)]
pub(crate) struct RepoChange {
    pub kind: RepoKind,
    /// The repo's top folder.
    pub root: PathBuf,
    /// Files or folders, relative to `root`, that the change may write or remove. Paths
    /// the change leaves alone are fine (only changed ones are committed).
    pub paths: Vec<PathBuf>,
    /// The commit message, such as "Turn pdf-tools off for Codex in Atlas".
    pub message: String,
}

/// Writes the change's files. Runs on a blocking thread, under the git lock, after the
/// checks pass. Return a sentence for the owner on failure.
pub(crate) type Apply = Box<dyn FnOnce() -> Result<(), String> + Send>;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// At least one repo got a commit.
    Committed {
        kinds: Vec<RepoKind>,
        /// The first commit's sha, shown in the flash.
        sha: String,
        /// Whether a push after the commit worked, when one was attempted.
        pushed: Option<bool>,
    },
    /// The change wrote nothing new.
    Unchanged,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ChangeError {
    /// The hub is read-only (no writer configured).
    ReadOnly,
    /// No session, a stale CSRF token or a cross-origin request.
    Forbidden,
    /// A rule refused it or it failed; the sentence says why and what (if anything) was
    /// kept.
    Refused(String),
}

#[derive(Serialize)]
struct ErrorJson<'a> {
    error: &'a str,
}

impl ChangeError {
    /// The response for a refused change. The page script gets JSON `{error}` (and shows
    /// it in place, without posting again); a plain form post gets text.
    pub(crate) fn into_response(self, headers: &HeaderMap) -> Response {
        let (status, msg) = match &self {
            Self::ReadOnly => (StatusCode::NOT_FOUND, "This hub is read-only."),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "Your session ended. Sign in again; nothing changed.",
            ),
            Self::Refused(m) => (StatusCode::CONFLICT, m.as_str()),
        };
        if wants_json(headers) {
            return (status, Json(ErrorJson { error: msg })).into_response();
        }
        match self {
            Self::Forbidden => forbidden("Your session ended. Sign in again; nothing changed.\n"),
            _ => (status, format!("{msg}\n")).into_response(),
        }
    }
}

/// Checks the hub can write and the request carries a live session and its CSRF token
/// from this origin. Every handler that changes something calls it first, before reading
/// the store or checking the form, so a signed-out or cross-origin post does no work.
pub(crate) fn authorise(s: &AppState, headers: &HeaderMap, csrf: &str) -> Result<(), ChangeError> {
    match &s.writer {
        None => Err(ChangeError::ReadOnly),
        Some(w) if w.authorised(headers, csrf) => Ok(()),
        Some(_) => Err(ChangeError::Forbidden),
    }
}

/// The answer to a change's form that can't go ahead before the change starts (a field
/// is missing or names something Atlas doesn't know). The page script gets JSON `{error}`,
/// which it shows in place instead of posting the form again; a plain post gets the usual
/// error page.
pub(crate) fn form_error(headers: &HeaderMap, e: AppError) -> Response {
    if !wants_json(headers) {
        return e.into_response();
    }
    let (status, msg) = match &e {
        AppError::Bad(m) => (StatusCode::BAD_REQUEST, m.as_str()),
        AppError::NotFound => (
            StatusCode::NOT_FOUND,
            "That isn't in Atlas any more. Nothing changed.",
        ),
        AppError::Internal => (
            StatusCode::INTERNAL_SERVER_ERROR,
            "Something went wrong. Nothing changed.",
        ),
    };
    (status, Json(ErrorJson { error: msg })).into_response()
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
    authorise(s, headers, csrf)?;
    let w = Arc::clone(s.writer.as_ref().ok_or(ChangeError::ReadOnly)?);
    let push = w.settings().push_after_save;
    let writer = Arc::clone(&w);
    let outcome = tokio::task::spawn_blocking(move || run(&writer.git, push, &repos, apply))
        .await
        .unwrap_or_else(|_| {
            Err(ChangeError::Refused(
                "The change stopped unexpectedly.".to_owned(),
            ))
        });
    // Whatever happened, the next page should read the store as it now is.
    s.invalidate().await;
    outcome
}

/// Changes `atlas/placements.toml` alone, as one commit in the knowledge repo: the file
/// is read and written inside the change, under the git lock (see [`rewrite_placements`]).
pub(crate) async fn edit_placements(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    message: String,
    edit: impl FnOnce(&mut Placements) -> Result<(), String> + Send + 'static,
) -> Result<Outcome, ChangeError> {
    let root = s.sources.knowledge.clone();
    let repos = vec![RepoChange {
        kind: RepoKind::Knowledge,
        root: root.clone(),
        paths: vec![PathBuf::from(PLACEMENTS_FILE)],
        message,
    }];
    let apply: Apply = Box::new(move || rewrite_placements(&root, edit));
    commit_change(s, headers, csrf, repos, apply).await
}

/// The one way a change writes the placements file, for use inside an [`Apply`]: reads
/// it fresh, changes it with `edit`, and writes it back only when it differs, so a change
/// that changes nothing commits nothing (not even a new empty file). A file changed since
/// it was read, or one that doesn't parse, refuses the change.
pub(crate) fn rewrite_placements(
    knowledge: &Path,
    edit: impl FnOnce(&mut Placements) -> Result<(), String>,
) -> Result<(), String> {
    let fail = |e: atlas_fs::library::LibraryError| format!("{e}.");
    let (mut placements, hash) = atlas_fs::library::read_placements(knowledge).map_err(fail)?;
    let before = placements.clone();
    edit(&mut placements)?;
    if placements != before {
        atlas_fs::library::write_placements(knowledge, &placements, &hash).map_err(fail)?;
    }
    Ok(())
}

/// Opens each repo and checks the rules before anything is written: each repo once, the
/// branch, and no uncommitted changes in the touched paths.
fn open_checked(repos: &[RepoChange]) -> Result<Vec<atlas_git::Repo>, ChangeError> {
    let refuse = |m: String| ChangeError::Refused(m);
    let mut opened: Vec<atlas_git::Repo> = Vec::with_capacity(repos.len());
    for change in repos {
        let repo = atlas_git::Repo::open(&change.root).map_err(|e| refuse(e.to_string()))?;
        if opened.iter().any(|r| r.root() == repo.root()) {
            return Err(refuse(format!(
                "{} is listed twice in one change.",
                shown(&change.root)
            )));
        }
        if let Some(branch) = change.kind.required_branch() {
            let actual = repo.current_branch().map_err(|e| refuse(e.to_string()))?;
            if actual.as_deref() != Some(branch) {
                return Err(refuse(format!(
                    "{} is on {}, not {branch}. Changes commit to {branch}; switch branch first. Nothing changed.",
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
    Ok(opened)
}

/// The owner's sentence for a failed commit, saying which repos kept their part.
fn commit_failed(root: &Path, e: &atlas_git::GitError, committed_in: &[String]) -> String {
    use std::fmt::Write as _;
    let text = e.to_string();
    let mut msg = if text.contains("tell me who you are") || text.contains("empty ident") {
        format!(
            "{} has no git name and email set, so git won't commit. Set user.name and user.email in it, then try again.",
            shown(root)
        )
    } else {
        format!("The commit in {} failed: {text}", shown(root))
    };
    if committed_in.is_empty() {
        msg.push_str(" Nothing changed.");
    } else {
        let _ = write!(
            msg,
            " {} already has its part committed, so the item is in both places for now; nothing is lost.",
            committed_in.join(" and ")
        );
    }
    msg
}

/// The change itself, without the web parts, so it can be tested directly.
pub(crate) fn run(
    lock: &Mutex<()>,
    push_after_save: bool,
    repos: &[RepoChange],
    apply: Apply,
) -> Result<Outcome, ChangeError> {
    let refuse = |m: String| ChangeError::Refused(m);
    // A change that panicked was rolled back below, so a poisoned lock is safe to take.
    let _guard = lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let opened = open_checked(repos)?;
    // What every touched path holds now, so a failure puts back exactly these bytes
    // without needing git (whose index may be the reason the commit failed).
    let mut saved: Vec<Vec<(String, atlas_fs::library::Saved)>> = Vec::with_capacity(repos.len());
    for change in repos {
        let mut kept = Vec::with_capacity(change.paths.len());
        for rel in &change.paths {
            let rel = rel.to_string_lossy().into_owned();
            let s = atlas_fs::library::save_path(&change.root, &rel).map_err(|e| {
                refuse(format!(
                    "{} can't be changed safely ({e}); nothing changed.",
                    Path::new(&shown(&change.root)).join(&rel).display()
                ))
            })?;
            kept.push((rel, s));
        }
        saved.push(kept);
    }
    // Restores the repos from `from` on, each with its own paths: the saved bytes first,
    // then git's index for those paths when git can.
    let restore_from = |from: usize| {
        for ((repo, change), kept) in opened.iter().zip(repos).zip(&saved).skip(from) {
            for (rel, s) in kept {
                if let Err(e) = atlas_fs::library::put_back(&change.root, rel, s) {
                    tracing::error!(
                        "putting {rel} back in {} after a failed change: {e}",
                        shown(&change.root)
                    );
                }
            }
            let paths: Vec<&Path> = change.paths.iter().map(PathBuf::as_path).collect();
            if let Err(e) = repo.restore_paths(&paths) {
                tracing::warn!(
                    "git's index for {} after a failed change: {e}",
                    shown(&change.root)
                );
            }
        }
    };
    match catch_unwind(AssertUnwindSafe(apply)) {
        Ok(Ok(())) => {}
        Ok(Err(msg)) => {
            restore_from(0);
            return Err(refuse(format!("{msg} Nothing changed.")));
        }
        Err(_) => {
            restore_from(0);
            return Err(refuse(
                "The change stopped unexpectedly. Nothing changed.".to_owned(),
            ));
        }
    }
    let mut kinds = Vec::new();
    let mut first_sha = None;
    let mut committed_in: Vec<String> = Vec::new();
    for (i, (repo, change)) in opened.iter().zip(repos).enumerate() {
        let paths: Vec<&Path> = change.paths.iter().map(PathBuf::as_path).collect();
        match repo.commit_paths(&paths, &change.message, change.kind.required_branch()) {
            Ok(sha) => {
                first_sha.get_or_insert(sha);
                kinds.push(change.kind.clone());
                committed_in.push(shown(&change.root));
            }
            Err(atlas_git::GitError::NothingToCommit) => {}
            Err(e) => {
                restore_from(i);
                return Err(refuse(commit_failed(&change.root, &e, &committed_in)));
            }
        }
    }
    let Some(sha) = first_sha else {
        return Ok(Outcome::Unchanged);
    };
    let mut pushed = None;
    for (repo, change) in opened.iter().zip(repos) {
        if kinds.contains(&change.kind) && change.kind.may_push(push_after_save) {
            let ok = repo
                .push()
                .map_err(|e| tracing::warn!("push after change failed: {e}"))
                .is_ok();
            pushed = Some(pushed.unwrap_or(true) && ok);
        }
    }
    Ok(Outcome::Committed { kinds, sha, pushed })
}

#[derive(Serialize)]
struct Done {
    next: String,
    send: bool,
}

/// Adds the flash parameters of an edit save (`saved=<sha>` and `pushed`/`pushfailed`/
/// `public`, or `unchanged`) to `next`, keeping any `#fragment` at the end.
fn with_flash(next: &str, outcome: &Outcome) -> String {
    let next = safe_next(next);
    let (path, fragment) = match next.split_once('#') {
        Some((p, f)) => (p.to_owned(), format!("#{f}")),
        None => (next.clone(), String::new()),
    };
    let sep = if path.contains('?') { '&' } else { '?' };
    let params = match outcome {
        Outcome::Committed { kinds, sha, pushed } => {
            let short = &sha[..sha.len().min(10)];
            let extra = match pushed {
                Some(true) => "&pushed=1",
                Some(false) => "&pushfailed=1",
                None if kinds.contains(&RepoKind::PublicSkills) => "&public=1",
                None => "",
            };
            format!("saved={short}{extra}")
        }
        Outcome::Unchanged => "unchanged=1".to_owned(),
    };
    format!("{path}{sep}{params}{fragment}")
}

/// The answer to a change: JSON `{next, send}` for the page script (which then asks for
/// the passkey tap that sends it), or a redirect without script.
pub(crate) fn respond(
    s: &AppState,
    headers: &HeaderMap,
    outcome: &Outcome,
    next: &str,
) -> Response {
    let next = with_flash(next, outcome);
    let knowledge =
        matches!(outcome, Outcome::Committed { kinds, .. } if kinds.contains(&RepoKind::Knowledge));
    let send = knowledge && s.writer.as_ref().is_some_and(|w| w.settings().send_on_save);
    if wants_json(headers) {
        Json(Done { next, send }).into_response()
    } else {
        Redirect::to(&next).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::process::Command;

    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .current_dir(dir)
            .args(args)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    fn repo(identity: bool) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path();
        git(p, &["init", "--quiet", "--initial-branch=main"]);
        git(p, &["config", "commit.gpgsign", "false"]);
        git(p, &["config", "user.name", "Test"]);
        git(p, &["config", "user.email", "test@example.invalid"]);
        fs::create_dir_all(p.join("skills/a")).unwrap();
        fs::write(p.join("skills/a/SKILL.md"), "a\n").unwrap();
        fs::write(p.join("notes.md"), "mine\n").unwrap();
        git(p, &["add", "."]);
        git(p, &["commit", "--quiet", "-m", "init"]);
        if !identity {
            // A repo whose commits fail: no identity anywhere git looks.
            git(p, &["config", "--unset", "user.name"]);
            git(p, &["config", "--unset", "user.email"]);
            git(p, &["config", "user.useConfigOnly", "true"]);
        }
        dir
    }

    fn change(dir: &Path, kind: RepoKind, paths: &[&str]) -> RepoChange {
        RepoChange {
            kind,
            root: dir.to_path_buf(),
            paths: paths.iter().map(PathBuf::from).collect(),
            message: "Move a in Atlas".to_owned(),
        }
    }

    #[test]
    fn a_failed_second_commit_restores_only_its_own_paths() {
        let dest = repo(true);
        // The source's commit fails because git's index is locked, and the lock stays
        // while Atlas restores it, so the restore can't lean on git.
        let src = repo(true);
        // The owner has an uncommitted file in the source repo at a path the destination
        // part names; restoring the source must never touch it.
        fs::create_dir_all(src.path().join("skills/b")).unwrap();
        fs::write(src.path().join("skills/b/draft.md"), "unsaved\n").unwrap();
        let repos = vec![
            change(dest.path(), RepoKind::Knowledge, &["skills/b"]),
            change(
                src.path(),
                RepoKind::Project(atlas_core::kernel::Slug::new("p").unwrap()),
                &["skills/a"],
            ),
        ];
        let (d, s) = (dest.path().to_path_buf(), src.path().to_path_buf());
        let got = run(
            &Mutex::new(()),
            false,
            &repos,
            Box::new(move || {
                fs::create_dir_all(d.join("skills/b")).unwrap();
                fs::write(d.join("skills/b/SKILL.md"), "b\n").unwrap();
                fs::remove_dir_all(s.join("skills/a")).unwrap();
                fs::write(s.join(".git/index.lock"), "").unwrap();
                Ok(())
            }),
        );
        let Err(ChangeError::Refused(msg)) = got else {
            panic!("{got:?}")
        };
        assert!(msg.contains("already has its part committed"), "{msg}");
        fs::remove_file(src.path().join(".git/index.lock")).unwrap();
        assert_eq!(
            git(src.path(), &["status", "--porcelain", "--", "skills/a"]),
            "",
            "skills/a is exactly as committed"
        );
        // The source is back as HEAD has it, and the owner's draft is untouched.
        assert_eq!(
            fs::read_to_string(src.path().join("skills/a/SKILL.md")).unwrap(),
            "a\n"
        );
        assert_eq!(
            fs::read_to_string(src.path().join("skills/b/draft.md")).unwrap(),
            "unsaved\n"
        );
        // The destination kept its commit.
        assert!(git(dest.path(), &["log", "--oneline"]).contains("Move a in Atlas"));
    }

    #[test]
    fn a_failing_or_panicking_apply_changes_nothing() {
        let r = repo(true);
        let repos = vec![change(
            r.path(),
            RepoKind::Knowledge,
            &["skills/a", "atlas/placements.toml"],
        )];
        for panics in [false, true] {
            let p = r.path().to_path_buf();
            let got = run(
                &Mutex::new(()),
                false,
                &repos,
                Box::new(move || {
                    fs::remove_dir_all(p.join("skills/a")).unwrap();
                    assert!(!panics, "boom");
                    Err("It didn't work.".to_owned())
                }),
            );
            assert!(matches!(got, Err(ChangeError::Refused(_))), "{got:?}");
            assert_eq!(
                fs::read_to_string(r.path().join("skills/a/SKILL.md")).unwrap(),
                "a\n"
            );
            assert_eq!(git(r.path(), &["status", "--porcelain"]), "");
        }
    }

    #[test]
    fn dirty_paths_refuse_before_anything_is_written() {
        let r = repo(true);
        fs::write(r.path().join("notes.md"), "edited by hand\n").unwrap();
        let repos = vec![change(r.path(), RepoKind::Knowledge, &["notes.md"])];
        let got = run(
            &Mutex::new(()),
            false,
            &repos,
            Box::new(|| panic!("must not run")),
        );
        assert!(matches!(got, Err(ChangeError::Refused(m)) if m.contains("uncommitted")));
        assert_eq!(
            fs::read_to_string(r.path().join("notes.md")).unwrap(),
            "edited by hand\n"
        );
    }

    #[test]
    fn a_new_file_and_an_untouched_path_commit_together() {
        let r = repo(true);
        let repos = vec![change(
            r.path(),
            RepoKind::Knowledge,
            &["skills/a", "atlas/placements.toml"],
        )];
        let p = r.path().to_path_buf();
        let got = run(
            &Mutex::new(()),
            false,
            &repos,
            Box::new(move || {
                fs::write(p.join("skills/a/extra.md"), "x\n").unwrap();
                Ok(())
            }),
        )
        .unwrap();
        assert!(matches!(got, Outcome::Committed { pushed: None, .. }));
        assert_eq!(git(r.path(), &["status", "--porcelain"]), "");
    }

    #[test]
    fn flash_params_follow_edit_saves_and_stay_on_site() {
        let done = Outcome::Committed {
            kinds: vec![RepoKind::Knowledge],
            sha: "0123456789abcdef".to_owned(),
            pushed: None,
        };
        assert_eq!(
            with_flash("/skills#outside", &done),
            "/skills?saved=0123456789#outside"
        );
        assert_eq!(
            with_flash("/skills?scope=x", &Outcome::Unchanged),
            "/skills?scope=x&unchanged=1"
        );
        assert_eq!(
            with_flash("//evil.test", &Outcome::Unchanged),
            "/?unchanged=1"
        );
        let public = Outcome::Committed {
            kinds: vec![RepoKind::PublicSkills],
            sha: "ab".to_owned(),
            pushed: None,
        };
        assert_eq!(with_flash("/skills", &public), "/skills?saved=ab&public=1");
    }
}
