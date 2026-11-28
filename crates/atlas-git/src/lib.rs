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

/// The git trailer key that marks a commit the hub made.
pub const ATLAS_TRAILER_KEY: &str = "Atlas-Change";
/// The trailer every commit the hub makes ends with, so later reads can tell the owner's
/// own commits (made by hand) from Atlas's, whatever its subject says.
pub const ATLAS_TRAILER: &str = "Atlas-Change: hub";

/// `message` with [`ATLAS_TRAILER`] as its last paragraph (once).
#[must_use]
pub fn with_atlas_trailer(message: &str) -> String {
    if message.lines().any(|l| l.trim() == ATLAS_TRAILER) {
        return message.to_owned();
    }
    format!("{}\n\n{ATLAS_TRAILER}", message.trim_end())
}
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
    #[error("{0:?} is too large to read ({1} bytes)")]
    TooLarge(PathBuf, u64),
    #[error("git {0} timed out")]
    Timeout(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// How a file changed between two commits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

/// One file that differs between two commits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangedPath {
    pub change: Change,
    /// From the repo's top, with `/` separators.
    pub path: String,
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

    /// True if git ignores `rel` (`.gitignore`, `.git/info/exclude`, the global excludes
    /// file). A tracked file is never ignored.
    ///
    /// # Errors
    /// `InvalidPath` for an absolute or escaping path; otherwise git failures.
    pub fn is_ignored(&self, rel: &Path) -> Result<bool, GitError> {
        check_rel(rel)?;
        // check-ignore takes pathnames and refuses --literal-pathspecs; it accepts no
        // pathspec magic but `:/`, which only means the top folder.
        let args = [os("check-ignore"), os("-q"), os("--"), rel.as_os_str()];
        let out = self.run_with(&args, LOCAL_TIMEOUT, false, |_| {})?;
        match out.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(command_error(&args, &out)),
        }
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
        let message = with_atlas_trailer(message);
        let commit = [
            os("commit"),
            os("--only"),
            os("--no-verify"),
            os("--quiet"),
            os("-m"),
            os(&message),
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

    /// Commits exactly the working-tree state of `paths` (files or folders, including
    /// additions and deletions under them) as one commit. Other changes in the repo stay
    /// as they were and are not part of the commit. For changes that span files, such as
    /// creating, moving or deleting a skill folder.
    ///
    /// # Errors
    /// `InvalidPath`, `WrongBranch` (checked before anything is changed),
    /// `NothingToCommit` if none of `paths` changed, or git failures (the index is then
    /// put back as it was for those paths).
    pub fn commit_paths(
        &self,
        paths: &[&Path],
        message: &str,
        require_branch: Option<&str>,
    ) -> Result<String, GitError> {
        if paths.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        for rel in paths {
            check_rel(rel)?;
        }
        if let Some(expected) = require_branch {
            let actual = self.current_branch()?;
            if actual.as_deref() != Some(expected) {
                return Err(GitError::WrongBranch {
                    expected: expected.to_owned(),
                    actual,
                });
            }
        }
        // Only paths that changed: git refuses a pathspec that matches nothing, such as a
        // file the change could have written but didn't.
        let mut dirty: Vec<&Path> = Vec::with_capacity(paths.len());
        for rel in paths {
            if !self.status_of(rel)?.is_empty() {
                dirty.push(rel);
            }
        }
        if dirty.is_empty() {
            return Err(GitError::NothingToCommit);
        }
        let with_paths = |head: &[&'static str]| -> Vec<&OsStr> {
            head.iter()
                .map(|s| os(s))
                .chain(dirty.iter().map(|p| p.as_os_str()))
                .collect()
        };
        self.git(&with_paths(&["add", "-A", "--"]), LOCAL_TIMEOUT)?;
        let mut commit: Vec<&OsStr> = ["commit", "--no-verify", "--quiet", "-m"]
            .iter()
            .map(|s| os(s))
            .collect();
        let message = with_atlas_trailer(message);
        commit.push(os(&message));
        commit.push(os("--"));
        commit.extend(dirty.iter().map(|p| p.as_os_str()));
        if let Err(err) = self.git(&commit, LOCAL_TIMEOUT) {
            // Best effort: leave the index for these paths as it was.
            let undo: &[&'static str] = if self.head_exists()? {
                &["reset", "-q", "HEAD", "--"]
            } else {
                &["rm", "-r", "-q", "--cached", "--ignore-unmatch", "--"]
            };
            let _ = self.run(&with_paths(undo), LOCAL_TIMEOUT);
            return Err(err);
        }
        let out = self.git(&["rev-parse", "HEAD"], LOCAL_TIMEOUT)?;
        Ok(trimmed(&out.stdout))
    }

    /// Puts `paths` back exactly as HEAD has them: tracked files restored, files git
    /// doesn't track removed, the index for them reset. Only for paths that were clean
    /// before a change began, so nothing of the owner's is lost.
    ///
    /// # Errors
    /// `InvalidPath`, or git failures.
    pub fn restore_paths(&self, paths: &[&Path]) -> Result<(), GitError> {
        for rel in paths {
            check_rel(rel)?;
        }
        let head = self.head_exists()?;
        // Every path is attempted even when one fails; the first error is returned.
        let mut first_err = None;
        for rel in paths {
            if let Err(e) = self.restore_one(rel, head) {
                first_err.get_or_insert(e);
            }
        }
        first_err.map_or(Ok(()), Err)
    }

    fn restore_one(&self, rel: &Path, head: bool) -> Result<(), GitError> {
        let undo_index = if head {
            [os("reset"), os("-q"), os("HEAD"), os("--")].to_vec()
        } else {
            [
                os("rm"),
                os("-r"),
                os("-q"),
                os("--cached"),
                os("--ignore-unmatch"),
                os("--"),
            ]
            .to_vec()
        };
        let mut undo_index = undo_index;
        undo_index.push(rel.as_os_str());
        let _ = self.run(&undo_index, LOCAL_TIMEOUT);
        let in_head = head && {
            let listed = self.git(
                &[
                    os("ls-tree"),
                    os("--name-only"),
                    os("HEAD"),
                    os("--"),
                    rel.as_os_str(),
                ],
                LOCAL_TIMEOUT,
            )?;
            !listed.stdout.is_empty()
        };
        // The path was clean before the change, so untracked files under it are the
        // change's. Remove them first (git clean spares ignored files, which may be the
        // owner's), so a file the change turned into a folder or back can be checked out.
        self.git(
            &[os("clean"), os("-fdq"), os("--"), rel.as_os_str()],
            LOCAL_TIMEOUT,
        )?;
        if in_head {
            self.git(
                &[os("checkout"), os("HEAD"), os("--"), rel.as_os_str()],
                LOCAL_TIMEOUT,
            )?;
        }
        Ok(())
    }

    /// Clones `remote` into `dir` (no checkout) the first time; afterwards opens the clone
    /// and points `origin` at `remote`. For a client's local copies of release sources.
    ///
    /// # Errors
    /// A remote that looks like an option, a failed clone, or a `dir` that isn't a clone.
    pub fn clone_or_open(remote: &str, dir: &Path) -> Result<Repo, GitError> {
        if remote.starts_with('-') || remote.contains(['\n', '\0']) || remote.starts_with("ext::") {
            return Err(GitError::InvalidPath(PathBuf::from(remote)));
        }
        if !dir.join(".git").exists() {
            let parent = dir
                .parent()
                .ok_or_else(|| GitError::InvalidPath(dir.to_path_buf()))?;
            std::fs::create_dir_all(parent)?;
            let at = Repo {
                root: parent.to_path_buf(),
            };
            at.git(
                &[
                    os("clone"),
                    os("--quiet"),
                    os("--no-checkout"),
                    os("--"),
                    os(remote),
                    dir.as_os_str(),
                ],
                PUSH_TIMEOUT,
            )?;
        }
        let repo = Repo::open(dir)?;
        // Only when it changed: `set-url` rewrites the config file even when it's the same.
        if repo.config("remote.origin.url")?.as_deref() != Some(remote) {
            repo.git(&["remote", "set-url", "origin", remote], LOCAL_TIMEOUT)?;
        }
        Ok(repo)
    }

    /// Fetches `commit` from `origin` unless it's already here.
    ///
    /// # Errors
    /// Not a full commit id, or the remote doesn't have it.
    pub fn fetch_commit(&self, commit: &str) -> Result<(), GitError> {
        check_oid(commit)?;
        let have = self.run(
            &["cat-file", "-e", &format!("{commit}^{{commit}}")],
            LOCAL_TIMEOUT,
        )?;
        if have.status.success() {
            return Ok(());
        }
        // Most hosts serve a commit by id; otherwise fetch the branches and look again.
        if self
            .git(
                &["fetch", "--quiet", "--no-tags", "origin", commit],
                PUSH_TIMEOUT,
            )
            .is_err()
        {
            self.git(&["fetch", "--quiet", "--no-tags", "origin"], PUSH_TIMEOUT)?;
        }
        Ok(())
    }

    /// Checks out `commit` detached, discarding anything else in the working tree.
    /// Only for Atlas's own copies, never a user's repo. A copy already detached at
    /// `commit` with nothing changed, staged, untracked or ignored is left alone, so a run
    /// with nothing new writes nothing; a copy that drifted is checked out and cleaned.
    ///
    /// # Errors
    /// Not a full commit id, or git fails.
    pub fn checkout_detached(&self, commit: &str) -> Result<(), GitError> {
        check_oid(commit)?;
        if self.pristine_at(commit) {
            return Ok(());
        }
        self.git(
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "--quiet",
                "--force",
                "--detach",
                commit,
            ],
            LOCAL_TIMEOUT,
        )?;
        self.git(&["clean", "-ffdxq"], LOCAL_TIMEOUT)?;
        Ok(())
    }

    /// Whether HEAD is detached at `commit` and the index and working tree match it
    /// exactly, which is what `checkout --force --detach` plus `clean -ffdx` would leave.
    /// One `git status` that takes no lock and doesn't refresh the index, so it writes
    /// nothing. Any doubt (git fails, odd output) counts as not pristine.
    fn pristine_at(&self, commit: &str) -> bool {
        let Ok(out) = self.run(
            &[
                "--no-optional-locks",
                "status",
                "--porcelain=v2",
                "--branch",
                "--untracked-files=all",
                "--ignored=matching",
                "--ignore-submodules=none",
            ],
            LOCAL_TIMEOUT,
        ) else {
            return false;
        };
        if !out.status.success() {
            return false;
        }
        let oid = format!("branch.oid {commit}");
        let (mut at, mut detached) = (false, false);
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            match line.strip_prefix("# ") {
                Some(h) if h == oid => at = true,
                Some("branch.head (detached)") => detached = true,
                Some(_) => {}
                // Any entry is a change, an untracked file or an ignored one.
                None => return false,
            }
        }
        at && detached
    }

    /// The tree of `commit`.
    ///
    /// # Errors
    /// Not a full commit id, or it isn't here.
    pub fn tree_of(&self, commit: &str) -> Result<String, GitError> {
        check_oid(commit)?;
        let out = self.git(
            &["rev-parse", "--verify", &format!("{commit}^{{tree}}")],
            LOCAL_TIMEOUT,
        )?;
        Ok(trimmed(&out.stdout))
    }

    /// How many commits HEAD is ahead of its upstream: `Some(0)` when pushed, None when
    /// the branch tracks nothing.
    ///
    /// # Errors
    /// If git fails for another reason.
    pub fn unpushed(&self) -> Result<Option<u32>, GitError> {
        let out = self.run(&["rev-list", "--count", "@{upstream}..HEAD"], LOCAL_TIMEOUT)?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(trimmed(&out.stdout).parse().ok())
    }

    /// The commit the branch's upstream points at and its tree: what a machine fetching
    /// the remote would get. None when the branch tracks nothing.
    ///
    /// # Errors
    /// If git fails for another reason.
    pub fn upstream_head(&self) -> Result<Option<(String, String)>, GitError> {
        let out = self.run(
            &["rev-parse", "@{upstream}", "@{upstream}^{tree}"],
            LOCAL_TIMEOUT,
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines().map(str::trim);
        Ok(match (lines.next(), lines.next()) {
            (Some(c), Some(t)) if !c.is_empty() && !t.is_empty() => {
                Some((c.to_owned(), t.to_owned()))
            }
            _ => None,
        })
    }

    /// How many commits HEAD has that `base` doesn't. None if `base` isn't known here.
    ///
    /// # Errors
    /// Not a full commit id.
    pub fn commits_since(&self, base: &str) -> Result<Option<u32>, GitError> {
        check_oid(base)?;
        let out = self.run(
            &["rev-list", "--count", &format!("{base}..HEAD")],
            LOCAL_TIMEOUT,
        )?;
        if !out.status.success() {
            return Ok(None);
        }
        Ok(trimmed(&out.stdout).parse().ok())
    }

    /// The commit HEAD points at and its tree, as full object ids.
    ///
    /// # Errors
    /// No commits yet, or git fails.
    pub fn head(&self) -> Result<(String, String), GitError> {
        let out = self.git(&["rev-parse", "HEAD", "HEAD^{tree}"], LOCAL_TIMEOUT)?;
        let text = String::from_utf8_lossy(&out.stdout);
        let mut lines = text.lines().map(str::trim);
        match (lines.next(), lines.next()) {
            (Some(commit), Some(tree)) if !commit.is_empty() && !tree.is_empty() => {
                Ok((commit.to_owned(), tree.to_owned()))
            }
            _ => Err(GitError::Command {
                args: vec!["rev-parse".into(), "HEAD".into()],
                stderr: "no HEAD".into(),
            }),
        }
    }

    /// The URL of the remote the current branch tracks, else `origin`'s. None when the
    /// repo has neither.
    ///
    /// # Errors
    /// If git fails.
    pub fn remote_url(&self) -> Result<Option<String>, GitError> {
        let branch = self.current_branch()?;
        let tracked = match &branch {
            Some(b) => self.config_value(&format!("branch.{b}.remote"))?,
            None => None,
        };
        let remote = tracked.unwrap_or_else(|| "origin".to_owned());
        self.config_value(&format!("remote.{remote}.url"))
    }

    fn config_value(&self, key: &str) -> Result<Option<String>, GitError> {
        let out = self.run(&["config", "--get", key], LOCAL_TIMEOUT)?;
        Ok(out
            .status
            .success()
            .then(|| trimmed(&out.stdout))
            .filter(|v| !v.is_empty()))
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
        let out = self.run_with(&args, PUSH_TIMEOUT, true, |cmd| {
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

    /// Whether `commit` is a commit this repo has.
    #[must_use]
    pub fn has_commit(&self, commit: &str) -> bool {
        check_oid(commit).is_ok()
            && self
                .run(
                    &["cat-file", "-e", &format!("{commit}^{{commit}}")],
                    LOCAL_TIMEOUT,
                )
                .is_ok_and(|o| o.status.success())
    }

    /// What differs between `base` and `head`, file by file (renames show as a delete and
    /// an add). With no `base`, every file in `head` counts as added.
    ///
    /// # Errors
    /// Not full commit ids, or git fails.
    pub fn changed_paths(
        &self,
        base: Option<&str>,
        head: &str,
    ) -> Result<Vec<ChangedPath>, GitError> {
        check_oid(head)?;
        let Some(base) = base else {
            let out = self.git(
                &["ls-tree", "-r", "-z", "--name-only", "--full-tree", head],
                LOCAL_TIMEOUT,
            )?;
            return Ok(nul_split(&out.stdout)
                .map(|path| ChangedPath {
                    change: Change::Added,
                    path,
                })
                .collect());
        };
        check_oid(base)?;
        let out = self.git(
            &[
                "diff",
                "--name-status",
                "-z",
                "--no-renames",
                "--no-ext-diff",
                base,
                head,
            ],
            LOCAL_TIMEOUT,
        )?;
        let mut parts = nul_split(&out.stdout);
        let mut changed = Vec::new();
        while let (Some(status), Some(path)) = (parts.next(), parts.next()) {
            let change = match status.as_bytes().first() {
                Some(b'A') => Change::Added,
                Some(b'D') => Change::Deleted,
                _ => Change::Modified,
            };
            changed.push(ChangedPath { change, path });
        }
        Ok(changed)
    }

    /// The bytes of the file at `rel` in `commit`. None when there is no file there.
    ///
    /// # Errors
    /// `InvalidPath`, `TooLarge` past `max` bytes, or git fails.
    pub fn file_at(&self, commit: &str, rel: &str, max: u64) -> Result<Option<Vec<u8>>, GitError> {
        check_oid(commit)?;
        check_rel(Path::new(rel))?;
        let object = format!("{commit}:{rel}");
        let kind = self.run(&["cat-file", "-t", &object], LOCAL_TIMEOUT)?;
        if !kind.status.success() || trimmed(&kind.stdout) != "blob" {
            return Ok(None);
        }
        let size = self.git(&["cat-file", "-s", &object], LOCAL_TIMEOUT)?;
        let size: u64 = trimmed(&size.stdout).parse().unwrap_or(u64::MAX);
        if size > max {
            return Err(GitError::TooLarge(PathBuf::from(rel), size));
        }
        let out = self.git(&["cat-file", "blob", &object], LOCAL_TIMEOUT)?;
        Ok(Some(out.stdout))
    }

    /// Every file under folder `rel` in `commit` with its tree mode (`100644`, `100755`,
    /// `120000` for a link), as paths from the repo's top.
    ///
    /// # Errors
    /// `InvalidPath`, or git fails.
    pub fn tree_entries(&self, commit: &str, rel: &str) -> Result<Vec<(String, String)>, GitError> {
        check_oid(commit)?;
        check_rel(Path::new(rel))?;
        let out = self.git(
            &[
                os("ls-tree"),
                os("-r"),
                os("-z"),
                os("--full-tree"),
                os(commit),
                os("--"),
                os(rel),
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(nul_split(&out.stdout)
            .filter_map(|entry| {
                let (meta, path) = entry.split_once('\t')?;
                Some((meta.split(' ').next()?.to_owned(), path.to_owned()))
            })
            .collect())
    }

    /// The tree mode of the file at `rel` in `commit`, such as `100755` for an executable
    /// file. None when there is nothing there.
    ///
    /// # Errors
    /// `InvalidPath`, or git fails.
    pub fn mode_at(&self, commit: &str, rel: &str) -> Result<Option<String>, GitError> {
        check_oid(commit)?;
        check_rel(Path::new(rel))?;
        let out = self.git(
            &[
                os("ls-tree"),
                os("-z"),
                os("--full-tree"),
                os(commit),
                os("--"),
                os(rel),
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(nul_split(&out.stdout).find_map(|entry| {
            let (meta, path) = entry.split_once('\t')?;
            (path == rel).then(|| meta.split(' ').next().unwrap_or_default().to_owned())
        }))
    }

    /// A unified diff of `rel` (a file or folder) from `base` to `head`, without colour,
    /// external diff tools or text conversion.
    ///
    /// # Errors
    /// `InvalidPath`, or git fails.
    pub fn diff(&self, base: &str, head: &str, rel: &str) -> Result<String, GitError> {
        check_oid(base)?;
        check_oid(head)?;
        check_rel(Path::new(rel))?;
        let out = self.git(
            &[
                os("diff"),
                os("--no-color"),
                os("--no-ext-diff"),
                os("--no-textconv"),
                os("--no-renames"),
                os(base),
                os(head),
                os("--"),
                os(rel),
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// The commits `head` has that `base` doesn't, newest first, as (id, subject, whether
    /// the hub made it: its message carries the [`ATLAS_TRAILER`]).
    ///
    /// # Errors
    /// Not full commit ids, or git fails.
    pub fn commits_between(
        &self,
        base: &str,
        head: &str,
    ) -> Result<Vec<(String, String, bool)>, GitError> {
        check_oid(base)?;
        check_oid(head)?;
        let format = format!(
            "--format=%H%x09%(trailers:key={ATLAS_TRAILER_KEY},valueonly,separator=%x2C)%x09%s"
        );
        let out = self.git(
            &[
                "log",
                "--no-color",
                "--no-show-signature",
                &format,
                &format!("{base}..{head}"),
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(|l| {
                let (id, rest) = l.split_once('\t')?;
                let (trailer, subject) = rest.split_once('\t')?;
                Some((
                    id.to_owned(),
                    subject.to_owned(),
                    !trailer.trim().is_empty(),
                ))
            })
            .collect())
    }

    /// The files `commit` changed compared with its first parent.
    ///
    /// # Errors
    /// Not a full commit id, a commit without a parent, or git fails.
    pub fn paths_in_commit(&self, commit: &str) -> Result<Vec<String>, GitError> {
        check_oid(commit)?;
        let out = self.git(
            &[
                "diff",
                "--name-only",
                "-z",
                "--no-renames",
                "--no-ext-diff",
                &format!("{commit}^1"),
                commit,
            ],
            LOCAL_TIMEOUT,
        )?;
        Ok(nul_split(&out.stdout).collect())
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
        self.run_with(args, timeout, true, |_| {})
    }

    /// `literal_pathspecs` is off only for commands that refuse it and take plain
    /// pathnames, not pathspecs (`check-ignore`).
    fn run_with<S: AsRef<OsStr>>(
        &self,
        args: &[S],
        timeout: Duration,
        literal_pathspecs: bool,
        extra: impl FnOnce(&mut Command),
    ) -> Result<Output, GitError> {
        let mut cmd = Command::new("git");
        cmd.current_dir(&self.root);
        if literal_pathspecs {
            cmd.arg("--literal-pathspecs");
        }
        cmd.args([
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

/// Accepts only full lowercase hex object ids, so nothing reaches git as an option.
fn check_oid(oid: &str) -> Result<(), GitError> {
    let hex = oid
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    if hex && matches!(oid.len(), 40 | 64) {
        Ok(())
    } else {
        Err(GitError::InvalidPath(PathBuf::from(oid)))
    }
}

/// Accepts only non-empty relative paths made of plain components.
fn check_rel(rel: &Path) -> Result<(), GitError> {
    let plain = rel.components().all(|c| matches!(c, Component::Normal(_)));
    if rel.as_os_str().is_empty() || !plain {
        return Err(GitError::InvalidPath(rel.to_path_buf()));
    }
    Ok(())
}

/// The non-empty NUL-separated fields of `-z` output.
fn nul_split(bytes: &[u8]) -> impl Iterator<Item = String> + '_ {
    bytes
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8_lossy(p).into_owned())
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
