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
        "Test <test@example.invalid>\nEdit a\n\nAtlas-Change: hub"
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
fn ignored_names_what_git_ignores() {
    let dir = fixture();
    let p = dir.path();
    fs::write(p.join(".gitignore"), "*.local.md\na.md\n").unwrap();
    fs::write(p.join(".git/info/exclude"), "/notes.txt\n").unwrap();
    for f in ["CLAUDE.local.md", "notes.txt", "kept.md"] {
        fs::write(p.join(f), "x\n").unwrap();
    }
    fs::create_dir_all(p.join(".claude")).unwrap();
    fs::write(p.join(".claude/CLAUDE.local.md"), "x\n").unwrap();
    let repo = Repo::open(p).unwrap();
    let ignored = |rel: &str| repo.is_ignored(Path::new(rel)).unwrap();
    assert!(ignored("CLAUDE.local.md"));
    assert!(ignored(".claude/CLAUDE.local.md"));
    assert!(ignored("notes.txt"));
    assert!(!ignored("kept.md"));
    assert!(!ignored("missing.md"));
    // Tracked, so not ignored although a pattern matches it.
    assert!(!ignored("a.md"));
    assert!(matches!(
        repo.is_ignored(Path::new("../x")),
        Err(GitError::InvalidPath(_))
    ));
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

#[test]
fn a_client_copy_follows_a_pinned_commit() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let git = |dir: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    git(&src, &["init", "-q", "-b", "main"]);
    git(&src, &["config", "user.name", "T"]);
    git(&src, &["config", "user.email", "t@example.com"]);
    std::fs::write(src.join("a"), "one").unwrap();
    git(&src, &["add", "a"]);
    git(&src, &["commit", "-qm", "one"]);
    let first = git(&src, &["rev-parse", "HEAD"]);
    std::fs::write(src.join("a"), "two").unwrap();
    git(&src, &["commit", "-qam", "two"]);
    let second = git(&src, &["rev-parse", "HEAD"]);

    let copy = tmp.path().join("cache/src");
    let remote = src.to_str().unwrap();
    let repo = atlas_git::Repo::clone_or_open(remote, &copy).unwrap();
    repo.fetch_commit(&first).unwrap();
    repo.checkout_detached(&first).unwrap();
    assert_eq!(std::fs::read_to_string(copy.join("a")).unwrap(), "one");
    assert_eq!(
        repo.tree_of(&first).unwrap(),
        git(&src, &["rev-parse", &format!("{first}^{{tree}}")])
    );
    std::fs::write(copy.join("stray"), "x").unwrap();
    let repo = atlas_git::Repo::clone_or_open(remote, &copy).unwrap();
    repo.fetch_commit(&second).unwrap();
    repo.checkout_detached(&second).unwrap();
    assert_eq!(std::fs::read_to_string(copy.join("a")).unwrap(), "two");
    assert!(!copy.join("stray").exists());

    // Already there and untouched: nothing is written, not even the index or the config.
    let written = |p: &str| std::fs::metadata(copy.join(p)).unwrap().modified().unwrap();
    let (index, config) = (written(".git/index"), written(".git/config"));
    std::thread::sleep(std::time::Duration::from_millis(20));
    let repo = atlas_git::Repo::clone_or_open(remote, &copy).unwrap();
    repo.checkout_detached(&second).unwrap();
    assert_eq!(
        (written(".git/index"), written(".git/config")),
        (index, config)
    );

    // Drift of any kind is still put right: an edit, a staged change, an untracked or an
    // ignored file, and a branch at the same commit.
    std::fs::write(copy.join("a"), "edited").unwrap();
    repo.checkout_detached(&second).unwrap();
    assert_eq!(std::fs::read_to_string(copy.join("a")).unwrap(), "two");
    std::fs::write(copy.join("new"), "x").unwrap();
    git(&copy, &["add", "new"]);
    repo.checkout_detached(&second).unwrap();
    assert!(!copy.join("new").exists());
    std::fs::write(copy.join(".git/info/exclude"), "*.log\n").unwrap();
    std::fs::write(copy.join("x.log"), "x").unwrap();
    repo.checkout_detached(&second).unwrap();
    assert!(!copy.join("x.log").exists());
    git(&copy, &["switch", "-q", "-c", "mine"]);
    repo.checkout_detached(&second).unwrap();
    assert_eq!(git(&copy, &["branch", "--show-current"]), "");
    assert!(repo.checkout_detached("--help").is_err());
    assert!(atlas_git::Repo::clone_or_open("--upload-pack=x", &tmp.path().join("bad")).is_err());
}

#[test]
fn upstream_and_waiting_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let bare = tmp.path().join("remote.git");
    let dir = tmp.path().join("work");
    let git = |at: &std::path::Path, args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(at)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    };
    std::fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    std::fs::create_dir_all(&dir).unwrap();
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.name", "T"]);
    git(&dir, &["config", "user.email", "t@example.com"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "one"]);
    let repo = atlas_git::Repo::open(&dir).unwrap();
    assert_eq!(repo.upstream_head().unwrap(), None);
    git(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&dir, &["push", "-q", "-u", "origin", "main"]);
    let one = git(&dir, &["rev-parse", "HEAD"]);
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "two"]);
    let (up, _) = repo.upstream_head().unwrap().unwrap();
    assert_eq!(up, one, "the upstream is what machines can fetch");
    assert_eq!(repo.commits_since(&one).unwrap(), Some(1));
    assert_eq!(repo.commits_since(&"0".repeat(40)).unwrap(), None);
}

#[test]
fn restore_puts_paths_back_as_head_has_them() {
    let dir = fixture();
    let p = dir.path();
    fs::create_dir_all(p.join("skills/kept")).unwrap();
    fs::write(p.join("skills/kept/SKILL.md"), "kept\n").unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "--quiet", "-m", "add kept"]);
    // A half-done change: a tracked folder deleted, a new one written and staged.
    fs::remove_dir_all(p.join("skills/kept")).unwrap();
    fs::create_dir_all(p.join("skills/new")).unwrap();
    fs::write(p.join("skills/new/SKILL.md"), "new\n").unwrap();
    sh(p, &["add", "skills/new"]);
    fs::write(p.join("b.md"), "b dirty\n").unwrap();

    let repo = Repo::open(p).unwrap();
    repo.restore_paths(&[Path::new("skills/kept"), Path::new("skills/new")])
        .unwrap();

    assert_eq!(
        fs::read_to_string(p.join("skills/kept/SKILL.md")).unwrap(),
        "kept\n"
    );
    assert!(!p.join("skills/new").exists());
    assert_eq!(sh(p, &["status", "--porcelain"]), " M b.md\n");
}

#[test]
fn commits_a_folder_move_as_one_commit_and_leaves_the_rest() {
    let dir = fixture();
    let p = dir.path();
    fs::create_dir_all(p.join("skills/old")).unwrap();
    fs::write(p.join("skills/old/SKILL.md"), "old\n").unwrap();
    sh(p, &["add", "."]);
    sh(p, &["commit", "--quiet", "-m", "add old"]);
    // Move skills/old to skills/new, with unrelated staged and unstaged changes around.
    fs::create_dir_all(p.join("skills/new")).unwrap();
    fs::rename(p.join("skills/old/SKILL.md"), p.join("skills/new/SKILL.md")).unwrap();
    fs::remove_dir(p.join("skills/old")).unwrap();
    fs::write(p.join("b.md"), "b dirty\n").unwrap();
    fs::write(p.join("c.md"), "c staged\n").unwrap();
    sh(p, &["add", "c.md"]);

    let repo = Repo::open(p).unwrap();
    repo.commit_paths(
        &[Path::new("skills/old"), Path::new("skills/new")],
        "Move old",
        Some("main"),
    )
    .unwrap();

    let files = sh(
        p,
        &["show", "--no-renames", "--name-only", "--format=", "HEAD"],
    );
    assert!(
        files.contains("skills/old/SKILL.md") && files.contains("skills/new/SKILL.md"),
        "{files}"
    );
    assert!(
        !files.contains("b.md") && !files.contains("c.md"),
        "{files}"
    );
    let status = sh(p, &["status", "--porcelain"]);
    assert!(
        status.contains(" M b.md") && status.contains("M  c.md"),
        "{status}"
    );
    assert!(matches!(
        repo.commit_paths(&[Path::new("skills/new")], "again", None),
        Err(GitError::NothingToCommit)
    ));
    assert!(matches!(
        repo.commit_paths(&[Path::new("../x")], "bad", None),
        Err(GitError::InvalidPath(_))
    ));
}

#[test]
fn a_named_path_that_was_never_written_does_not_fail_the_commit() {
    let dir = fixture();
    let p = dir.path();
    fs::write(p.join("a.md"), "a v2\n").unwrap();
    let repo = Repo::open(p).unwrap();
    // A change may name a file it could write (the placements file) but didn't.
    repo.commit_paths(
        &[Path::new("a.md"), Path::new("atlas/placements.toml")],
        "Edit a",
        None,
    )
    .unwrap();
    assert_eq!(files_in(p, "HEAD").trim(), "a.md");
}

#[test]
fn reads_what_changed_between_two_commits() {
    let dir = fixture();
    let p = dir.path();
    let repo = Repo::open(p).unwrap();
    let base = sh(p, &["rev-parse", "HEAD"]).trim().to_owned();
    fs::write(p.join("a.md"), "a v2\n").unwrap();
    fs::remove_file(p.join("b.md")).unwrap();
    fs::create_dir_all(p.join("hooks/guard")).unwrap();
    fs::write(p.join("hooks/guard/run.sh"), "echo hi\n").unwrap();
    // Made through Atlas, with a subject that doesn't say so: the trailer does.
    repo.commit_paths(
        &[Path::new("a.md"), Path::new("b.md"), Path::new("hooks")],
        "Bring a into Atlas from elsewhere",
        None,
    )
    .unwrap();
    fs::write(p.join("c.md"), "c v2\n").unwrap();
    sh(p, &["commit", "--quiet", "-am", "by hand"]);
    let head = sh(p, &["rev-parse", "HEAD"]).trim().to_owned();

    let mut changed = repo.changed_paths(Some(&base), &head).unwrap();
    changed.sort_by(|a, b| a.path.cmp(&b.path));
    let got: Vec<_> = changed
        .iter()
        .map(|c| (c.change, c.path.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            (atlas_git::Change::Modified, "a.md"),
            (atlas_git::Change::Deleted, "b.md"),
            (atlas_git::Change::Modified, "c.md"),
            (atlas_git::Change::Added, "hooks/guard/run.sh"),
        ]
    );
    let all = repo.changed_paths(None, &head).unwrap();
    assert!(all.iter().all(|c| c.change == atlas_git::Change::Added));
    assert_eq!(all.len(), 3, "{all:?}");

    assert_eq!(
        repo.file_at(&head, "hooks/guard/run.sh", 1024)
            .unwrap()
            .unwrap(),
        b"echo hi\n"
    );
    assert!(
        repo.file_at(&base, "hooks/guard/run.sh", 1024)
            .unwrap()
            .is_none()
    );
    assert!(
        repo.file_at(&head, "hooks", 1024).unwrap().is_none(),
        "a folder"
    );
    assert!(matches!(
        repo.file_at(&head, "hooks/guard/run.sh", 3),
        Err(GitError::TooLarge(..))
    ));
    assert!(repo.file_at(&head, "../x", 1024).is_err());
    assert_eq!(
        repo.tree_entries(&head, "hooks/guard").unwrap(),
        vec![("100644".to_owned(), "hooks/guard/run.sh".to_owned())]
    );
    assert_eq!(
        repo.mode_at(&head, "hooks/guard/run.sh")
            .unwrap()
            .as_deref(),
        Some("100644")
    );
    let diff = repo.diff(&base, &head, "a.md").unwrap();
    assert!(
        diff.contains("-a.md v1") && diff.contains("+a v2"),
        "{diff}"
    );

    let commits = repo.commits_between(&base, &head).unwrap();
    let subjects: Vec<_> = commits.iter().map(|c| c.1.as_str()).collect();
    assert_eq!(
        subjects,
        vec!["by hand", "Bring a into Atlas from elsewhere"]
    );
    // Commits Atlas makes carry its trailer whatever their subject; the owner's don't.
    let by_atlas: Vec<bool> = commits.iter().map(|c| c.2).collect();
    assert_eq!(by_atlas, vec![false, true]);
    assert_eq!(repo.paths_in_commit(&commits[0].0).unwrap(), vec!["c.md"]);
    assert!(repo.has_commit(&base));
    assert!(!repo.has_commit(&"0".repeat(40)));
    assert!(
        repo.changed_paths(Some("HEAD"), &head).is_err(),
        "only full ids"
    );
}
