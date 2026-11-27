use std::fs;
use std::path::Path;
use std::process::Command;

use atlas_git::{GitError, Repo};
use tempfile::TempDir;

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A repo on branch `main` with a local identity and one commit of a, b, c.
fn fixture() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path();
    sh(p, &["init", "--quiet", "--initial-branch=main"]);
    sh(p, &["config", "user.name", "Test"]);
    sh(p, &["config", "user.email", "test@example.invalid"]);
    sh(p, &["config", "commit.gpgsign", "false"]);
    for f in ["a.md", "b.md", "c.md"] {
        fs::write(p.join(f), format!("{f} v1\n")).unwrap();
    }
    sh(p, &["add", "."]);
    sh(p, &["commit", "--quiet", "-m", "init"]);
    dir
}

fn files_in(dir: &Path, rev: &str) -> String {
    sh(dir, &["show", "--name-only", "--format=", rev])
}

#[test]
fn commits_only_the_file_and_leaves_other_changes_alone() {
    let dir = fixture();
    let p = dir.path();
    fs::write(p.join("a.md"), "a v2\n").unwrap();
    fs::write(p.join("b.md"), "b dirty\n").unwrap();
    fs::write(p.join("c.md"), "c staged\n").unwrap();
    sh(p, &["add", "c.md"]);
    let before = sh(p, &["status", "--porcelain"]);

    let repo = Repo::open(p).unwrap();
    let sha = repo
        .commit_file(Path::new("a.md"), "Edit a", Some("main"))
        .unwrap();

    assert_eq!(sha, sh(p, &["rev-parse", "HEAD"]).trim());
    assert_eq!(sha.len(), 40);
    assert_eq!(files_in(p, "HEAD").trim(), "a.md");
    assert_eq!(sh(p, &["show", "HEAD:a.md"]), "a v2\n");
    assert_eq!(
        sh(p, &["log", "-1", "--format=%an <%ae>%n%B"]).trim(),
        "Test <test@example.invalid>\nEdit a"
    );
    let after = sh(p, &["status", "--porcelain"]);
    assert_eq!(after, " M b.md\nM  c.md\n");
    assert!(before.contains(" M b.md") && before.contains("M  c.md"));
    assert_eq!(sh(p, &["diff", "--cached", "--name-only"]).trim(), "c.md");
    assert!(repo.is_clean(Path::new("a.md")).unwrap());
    assert!(!repo.is_clean(Path::new("b.md")).unwrap());
    assert!(matches!(
        repo.ensure_clean(Path::new("c.md")),
        Err(GitError::Dirty(_))
    ));
}

#[test]
fn commits_a_new_untracked_file_alone() {
    let dir = fixture();
    let p = dir.path();
    fs::create_dir(p.join("notes")).unwrap();
    fs::write(p.join("notes/new.md"), "new\n").unwrap();
    fs::write(p.join("other.md"), "untracked too\n").unwrap();
    fs::write(p.join("c.md"), "c staged\n").unwrap();
    sh(p, &["add", "c.md"]);

    let repo = Repo::open(p).unwrap();
    assert!(!repo.is_clean(Path::new("notes/new.md")).unwrap());
    repo.commit_file(Path::new("notes/new.md"), "Add note", None)
        .unwrap();

    assert_eq!(files_in(p, "HEAD").trim(), "notes/new.md");
    assert_eq!(sh(p, &["status", "--porcelain"]), "M  c.md\n?? other.md\n");
}

#[test]
fn unchanged_file_is_nothing_to_commit() {
    let dir = fixture();
    let p = dir.path();
    let repo = Repo::open(p).unwrap();
    assert!(matches!(
        repo.commit_file(Path::new("a.md"), "noop", None),
        Err(GitError::NothingToCommit)
    ));
    // Staged change reverted in the working tree: the file itself equals HEAD.
    fs::write(p.join("a.md"), "a staged\n").unwrap();
    sh(p, &["add", "a.md"]);
    fs::write(p.join("a.md"), "a.md v1\n").unwrap();
    assert!(matches!(
        repo.commit_file(Path::new("a.md"), "noop", None),
        Err(GitError::NothingToCommit)
    ));
    assert_eq!(sh(p, &["rev-list", "--count", "HEAD"]).trim(), "1");
}

#[test]
fn wrong_branch_fails_before_committing() {
    let dir = fixture();
    let p = dir.path();
    sh(p, &["switch", "--quiet", "-c", "feature"]);
    fs::write(p.join("a.md"), "a v2\n").unwrap();
    let repo = Repo::open(p).unwrap();
    assert_eq!(repo.current_branch().unwrap().as_deref(), Some("feature"));
    match repo.commit_file(Path::new("a.md"), "Edit", Some("main")) {
        Err(GitError::WrongBranch { expected, actual }) => {
            assert_eq!(expected, "main");
            assert_eq!(actual.as_deref(), Some("feature"));
        }
        other => panic!("expected WrongBranch, got {other:?}"),
    }
    sh(p, &["switch", "--quiet", "--detach", "HEAD"]);
    assert_eq!(repo.current_branch().unwrap(), None);
    assert!(matches!(
        repo.commit_file(Path::new("a.md"), "Edit", Some("main")),
        Err(GitError::WrongBranch { actual: None, .. })
    ));
    assert_eq!(sh(p, &["rev-list", "--count", "HEAD"]).trim(), "1");
}

#[test]
fn open_refuses_subfolders_and_non_repos() {
    let dir = fixture();
    fs::create_dir(dir.path().join("sub")).unwrap();
    assert!(matches!(
        Repo::open(&dir.path().join("sub")),
        Err(GitError::NotARepo(_))
    ));
    let plain = tempfile::tempdir().unwrap();
    assert!(matches!(
        Repo::open(plain.path()),
        Err(GitError::NotARepo(_))
    ));
    assert!(matches!(
        Repo::open(&dir.path().join("missing")),
        Err(GitError::NotARepo(_))
    ));
    let repo = Repo::open(dir.path()).unwrap();
    assert_eq!(repo.root(), dir.path().canonicalize().unwrap());
}

#[test]
fn escaping_and_absolute_paths_are_refused() {
    let dir = fixture();
    let repo = Repo::open(dir.path()).unwrap();
    for bad in ["../a.md", "sub/../../x", "/etc/passwd", "", "./a.md"] {
        assert!(
            matches!(
                repo.commit_file(Path::new(bad), "x", None),
                Err(GitError::InvalidPath(_))
            ),
            "{bad} accepted"
        );
        assert!(matches!(
            repo.is_clean(Path::new(bad)),
            Err(GitError::InvalidPath(_))
        ));
    }
}

#[test]
fn pathspec_magic_is_taken_literally() {
    let dir = fixture();
    let p = dir.path();
    fs::write(p.join("b.md"), "b dirty\n").unwrap();
    fs::write(p.join(":(glob)*.md"), "odd name\n").unwrap();
    let repo = Repo::open(p).unwrap();
    repo.commit_file(Path::new(":(glob)*.md"), "odd", None)
        .unwrap();
    assert_eq!(files_in(p, "HEAD").trim(), ":(glob)*.md");
    assert_eq!(sh(p, &["status", "--porcelain"]), " M b.md\n");
}

#[test]
fn repo_hooks_never_run() {
    let dir = fixture();
    let p = dir.path();
    let marker = p.join("hook-ran");
    let hook = format!("#!/bin/sh\ntouch '{}'\nexit 1\n", marker.display());
    for name in ["pre-commit", "commit-msg", "post-commit", "pre-push"] {
        let path = p.join(".git/hooks").join(name);
        fs::write(&path, &hook).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }
    fs::write(p.join("a.md"), "a v2\n").unwrap();
    let repo = Repo::open(p).unwrap();
    repo.commit_file(Path::new("a.md"), "Edit", None).unwrap();
    assert!(!marker.exists(), "a repo hook ran");
    // Sanity check: the hook does run for a plain git commit.
    fs::write(p.join("b.md"), "b v2\n").unwrap();
    let plain = Command::new("git")
        .current_dir(p)
        .args(["commit", "-q", "-m", "x", "--", "b.md"])
        .output()
        .unwrap();
    assert!(!plain.status.success() && marker.exists());
}

#[test]
fn push_to_local_bare_remote() {
    let dir = fixture();
    let p = dir.path();
    let remote = tempfile::tempdir().unwrap();
    sh(remote.path(), &["init", "--quiet", "--bare"]);
    let url = remote.path().to_str().unwrap();
    sh(p, &["remote", "add", "origin", url]);
    sh(p, &["push", "--quiet", "-u", "origin", "main"]);

    fs::write(p.join("a.md"), "a v2\n").unwrap();
    let repo = Repo::open(p).unwrap();
    let sha = repo.commit_file(Path::new("a.md"), "Edit", None).unwrap();
    repo.push().unwrap();
    assert_eq!(sh(remote.path(), &["rev-parse", "main"]).trim(), sha);
}

#[test]
fn push_without_upstream_fails_fast() {
    let dir = fixture();
    let repo = Repo::open(dir.path()).unwrap();
    let start = std::time::Instant::now();
    assert!(matches!(
        repo.push(),
        Err(GitError::NoUpstream { branch: Some(b) }) if b == "main"
    ));
    sh(
        dir.path(),
        &["remote", "add", "origin", "/nonexistent/remote.git"],
    );
    sh(dir.path(), &["config", "branch.main.remote", "origin"]);
    sh(
        dir.path(),
        &["config", "branch.main.merge", "refs/heads/main"],
    );
    assert!(matches!(repo.push(), Err(GitError::Command { .. })));
    assert!(start.elapsed().as_secs() < 10);
}

#[test]
fn head_and_remote_are_read() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("r");
    std::fs::create_dir_all(&dir).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(&dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "{args:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&["init", "-q", "-b", "main"]);
    git(&["config", "user.name", "T"]);
    git(&["config", "user.email", "t@example.com"]);
    std::fs::write(dir.join("a"), "a").unwrap();
    git(&["add", "a"]);
    git(&["commit", "-qm", "a"]);
    let repo = atlas_git::Repo::open(&dir).unwrap();
    assert_eq!(repo.remote_url().unwrap(), None);
    git(&["remote", "add", "origin", "https://git.example/r.git"]);
    assert_eq!(
        repo.remote_url().unwrap().as_deref(),
        Some("https://git.example/r.git")
    );
    let (commit, tree) = repo.head().unwrap();
    assert_eq!(commit, git(&["rev-parse", "HEAD"]));
    assert_eq!(tree, git(&["rev-parse", "HEAD^{tree}"]));
}
