//! Drives the `git` CLI so the hub can commit one edited file and push it.
//!
//! Every command runs without hooks, without prompts and with literal pathspecs,
//! because the repositories may hold hostile content.

use std::ffi::OsStr;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

const LOCAL_TIMEOUT: Duration = Duration::from_secs(30);
const PUSH_TIMEOUT: Duration = Duration::from_secs(90);

/// Environment variables that could point git at another repo, index or object store.
const CLEARED_ENV: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_COMMON_DIR",
    "GIT_NAMESPACE",
    "GIT_PREFIX",
];

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("{0} is not the top level of a git repository")]
    NotARepo(PathBuf),
    #[error("{0} has uncommitted changes")]
    Dirty(PathBuf),
    #[error("path {0} must be relative and stay inside the repository")]
    InvalidPath(PathBuf),
    #[error("expected branch {expected}, but HEAD is {}", actual.as_deref().unwrap_or("detached"))]
    WrongBranch {
        expected: String,
        actual: Option<String>,
    },
    #[error("nothing to commit")]
    NothingToCommit,
    #[error("branch {} has no upstream to push to", branch.as_deref().unwrap_or("(detached HEAD)"))]
    NoUpstream { branch: Option<String> },
    #[error("git {} failed: {stderr}", args.join(" "))]
    Command { args: Vec<String>, stderr: String },
    #[error("git {0} timed out")]
    Timeout(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A git working tree, addressed by its top-level directory.
#[derive(Debug, Clone)]
pub struct Repo {
    root: PathBuf,
}

impl Repo {
    /// Opens the repo whose top level is exactly `root` (canonicalised).
    ///
    /// # Errors
    /// `NotARepo` if `root` is not a repo, or is a subfolder of one.
    pub fn open(root: &Path) -> Result<Repo, GitError> {
        let root = root
            .canonicalize()
            .map_err(|_| GitError::NotARepo(root.to_path_buf()))?;
        let repo = Repo { root };
        let out = repo
            .run(&["rev-parse", "--show-toplevel"], LOCAL_TIMEOUT)
            .map_err(|_| GitError::NotARepo(repo.root.clone()))?;
        if !out.status.success() {
            return Err(GitError::NotARepo(repo.root));
        }
        let top = PathBuf::from(trimmed(&out.stdout));
        match top.canonicalize() {
            Ok(top) if top == repo.root => Ok(repo),
            _ => Err(GitError::NotARepo(repo.root)),
        }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Current branch name, `None` when HEAD is detached.
    ///
    /// # Errors
    /// `Command`, `Timeout` or `Io` if git fails.
    pub fn current_branch(&self) -> Result<Option<String>, GitError> {
        let args = ["symbolic-ref", "--quiet", "--short", "HEAD"];
        let out = self.run(&args, LOCAL_TIMEOUT)?;
        match out.status.code() {
            Some(0) => Ok(Some(trimmed(&out.stdout))),
            Some(1) => Ok(None),
            _ => Err(command_error(&args, &out)),
        }
    }

    /// True if `rel` has no staged or unstaged changes and is not untracked.
    ///
    /// # Errors
    /// `InvalidPath` for an absolute or escaping path; otherwise git failures.
    pub fn is_clean(&self, rel: &Path) -> Result<bool, GitError> {
        check_rel(rel)?;
        Ok(self.status_of(rel)?.is_empty())
    }

    /// Like [`Repo::is_clean`], but returns `Dirty` when `rel` has changes.
    ///
    /// # Errors
    /// `Dirty` if the file has uncommitted changes; otherwise as `is_clean`.
    pub fn ensure_clean(&self, rel: &Path) -> Result<(), GitError> {
        if self.is_clean(rel)? {
            Ok(())
        } else {
            Err(GitError::Dirty(rel.to_path_buf()))
        }
    }

    /// Commits exactly `rel` (its working-tree content) with `message`.
    ///
    /// Other staged and unstaged changes stay as they were and are not part of the
    /// commit. Uses the repo's configured identity. Returns the new commit's sha.
    ///
    /// # Errors
    /// `InvalidPath`, `WrongBranch` (checked before anything is changed),
    /// `NothingToCommit` if `rel` is unchanged, or git failures.
    pub fn commit_file(
        &self,
        rel: &Path,
        message: &str,
        require_branch: Option<&str>,
    ) -> Result<String, GitError> {
        check_rel(rel)?;
        if let Some(expected) = require_branch {
            let actual = self.current_branch()?;
            if actual.as_deref() != Some(expected) {
                return Err(GitError::WrongBranch {
                    expected: expected.to_owned(),
                    actual,
                });
            }
        }
        if self.root.join(rel).is_dir() {
            return Err(GitError::InvalidPath(rel.to_path_buf()));
        }
        let status = self.status_of(rel)?;
        if status.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        let untracked = status.starts_with(b"??");
        // " M": nothing staged and the file differs from the index, which equals HEAD, so it
        // certainly differs from HEAD; only other states need the slower check.
        let plainly_modified = status.starts_with(b" M");
        if !untracked && !plainly_modified && self.head_exists()? && self.matches_head(rel)? {
            return Err(GitError::NothingToCommit);
        }
        if untracked {
            self.git(&[os("add"), os("--"), rel.as_os_str()], LOCAL_TIMEOUT)?;
        }
        let commit = [
            os("commit"),
            os("--only"),
            os("--no-verify"),
            os("--quiet"),
            os("-m"),
            os(message),
            os("--"),
            rel.as_os_str(),
        ];
        if let Err(err) = self.git(&commit, LOCAL_TIMEOUT) {
            if untracked {
                // Best effort: leave the index as we found it.
                let _ = self.run(
                    &[
                        os("rm"),
                        os("--cached"),
                        os("--quiet"),
                        os("--"),
                        rel.as_os_str(),
                    ],
                    LOCAL_TIMEOUT,
                );
            }
            return Err(err);
        }
        let out = self.git(&["rev-parse", "HEAD"], LOCAL_TIMEOUT)?;
        Ok(trimmed(&out.stdout))
    }

    /// Pushes the current branch to its configured upstream, never prompting.
    ///
    /// # Errors
    /// `NoUpstream` if HEAD is detached or the branch has no upstream;
    /// `Command` (e.g. rejected, no credentials) or `Timeout` after 90 s.
    pub fn push(&self) -> Result<(), GitError> {
        let branch = self.current_branch()?;
        let Some(name) = branch.clone() else {
            return Err(GitError::NoUpstream { branch });
        };
        let remote = self.config(&format!("branch.{name}.remote"))?;
        let merge = self.config(&format!("branch.{name}.merge"))?;
        let (Some(remote), Some(merge)) = (remote, merge) else {
            return Err(GitError::NoUpstream { branch });
        };
        if remote.starts_with('-') || remote == "." || merge.starts_with('-') {
            return Err(GitError::NoUpstream { branch });
        }
        let refspec = format!("refs/heads/{name}:{merge}");
        let args = [
            "push",
            "--porcelain",
            "--no-verify",
            remote.as_str(),
            refspec.as_str(),
        ];
        let batch_ssh = self.config("core.sshCommand")?.is_none()
            && std::env::var_os("GIT_SSH_COMMAND").is_none()
            && std::env::var_os("GIT_SSH").is_none();
        let out = self.run_with(&args, PUSH_TIMEOUT, |cmd| {
            if batch_ssh {
                cmd.env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes");
            }
        })?;
        if out.status.success() {
            Ok(())
        } else {
            Err(command_error(&args, &out))
        }
    }

    fn status_of(&self, rel: &Path) -> Result<Vec<u8>, GitError> {
        let out = self.git(
            &[
                os("status"),
                os("--porcelain=v1"),
                os("-z"),
                os("--untracked-files=all"),
                os("--ignored=no"),
                os("--"),
                rel.as_os_str(),
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(out.stdout)
    }

    fn head_exists(&self) -> Result<bool, GitError> {
        let out = self.run(&["rev-parse", "--verify", "--quiet", "HEAD"], LOCAL_TIMEOUT)?;
        Ok(out.status.success())
    }

    /// True when the working-tree content of `rel` equals HEAD's.
    fn matches_head(&self, rel: &Path) -> Result<bool, GitError> {
        let args = [
            os("diff"),
            os("--quiet"),
            os("--no-ext-diff"),
            os("--no-textconv"),
            os("HEAD"),
            os("--"),
            rel.as_os_str(),
        ];
        let out = self.run(&args, LOCAL_TIMEOUT)?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(command_error(&args, &out)),
        }
    }

    fn config(&self, key: &str) -> Result<Option<String>, GitError> {
        let args = ["config", "--get", key];
        let out = self.run(&args, LOCAL_TIMEOUT)?;
        match out.status.code() {
            Some(0) => Ok(Some(trimmed(&out.stdout))),
            Some(1) => Ok(None),
            _ => Err(command_error(&args, &out)),
        }
    }

    fn git<S: AsRef<OsStr>>(&self, args: &[S], timeout: Duration) -> Result<Output, GitError> {
        let out = self.run(args, timeout)?;
        if out.status.success() {
            Ok(out)
        } else {
            Err(command_error(args, &out))
        }
    }

    fn run<S: AsRef<OsStr>>(&self, args: &[S], timeout: Duration) -> Result<Output, GitError> {
        self.run_with(args, timeout, |_| {})
    }

    fn run_with<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        timeout: Duration,
        extra: impl FnOnce(&mut Command),
    ) -> Result<Output, GitError> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.root)
            .args([
                "--literal-pathspecs",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "core.editor=true",
                "-c",
                "core.fsmonitor=false",
            ])
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ASKPASS", "")
            .env("SSH_ASKPASS", "")
            .env("SSH_ASKPASS_REQUIRE", "never")
            .env("GCM_INTERACTIVE", "never")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in CLEARED_ENV {
            cmd.env_remove(var);
        }
        extra(&mut cmd);
        // Wait for git on a small per-thread runtime: the kernel tells us when it exits and
        // both pipes are read without extra threads; on timeout git is killed with it.
        let mut cmd = tokio::process::Command::from(cmd);
        cmd.kill_on_drop(true);
        let waited = RUNTIME.with(|rt| match rt {
            // Built inside the runtime: tokio starts the child as soon as `output` is called.
            Ok(rt) => Ok(rt.block_on(async { tokio::time::timeout(timeout, cmd.output()).await })),
            Err(e) => Err(GitError::Io(io::Error::other(e.to_string()))),
        })?;
        match waited {
            Ok(output) => Ok(output?),
            Err(_) => Err(GitError::Timeout(describe(args))),
        }
    }
}

thread_local! {
    /// One tiny runtime per blocking thread, reused across git commands. Callers run on
    /// `spawn_blocking` threads (or plain threads in tests), never inside async code.
    static RUNTIME: Result<tokio::runtime::Runtime, io::Error> =
        tokio::runtime::Builder::new_current_thread().enable_io().enable_time().build();
}

/// Accepts only non-empty relative paths made of plain components.
fn check_rel(rel: &Path) -> Result<(), GitError> {
    let plain = rel.components().all(|c| matches!(c, Component::Normal(_)));
    if rel.as_os_str().is_empty() || !plain {
        return Err(GitError::InvalidPath(rel.to_path_buf()));
    }
    Ok(())
}

fn os(s: &str) -> &OsStr {
    OsStr::new(s)
}

fn trimmed(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim_end_matches(['\n', '\r'])
        .to_owned()
}

fn describe<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter()
        .map(|a| a.as_ref().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_error<S: AsRef<OsStr>>(args: &[S], out: &Output) -> GitError {
    GitError::Command {
        args: args
            .iter()
            .map(|a| a.as_ref().to_string_lossy().into_owned())
            .collect(),
        stderr: String::from_utf8_lossy(&out.stderr).trim().to_owned(),
    }
}
