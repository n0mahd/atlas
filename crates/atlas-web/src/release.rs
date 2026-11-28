//! Updates (signed releases underneath) on the hub: build a manifest from the sources
//! machines use, keep signed ones, and hand the latest to paired clients. The hub never
//! signs (decisions 0007, 0022).

use std::path::{Path, PathBuf};

use atlas_core::kernel::TreeHash;
use atlas_core::release::{CommitId, FORMAT, Manifest, OwnerKey, SignedRelease, Source, SourceId};
use atlas_fs::Sources;

/// A repo left out of an update, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub source: String,
    pub reason: String,
}

/// Builds the next manifest from the sources machines use (Knowledge and public skills),
/// each pinned at its upstream's commit: that is what a machine can fetch. Project repos
/// stay out, because atlasd doesn't distribute them.
///
/// # Errors
/// A sentence for the owner when a source's remote address holds a password or token:
/// nothing is sent until it's removed.
pub fn build(
    sources: &Sources,
    number: u64,
    hub: &str,
    owner_keys: Vec<OwnerKey>,
    now: u64,
) -> Result<(Manifest, Vec<Skipped>), String> {
    let mut repos: Vec<(SourceId, PathBuf)> =
        vec![(SourceId::Knowledge, sources.knowledge.clone())];
    if let Some(p) = &sources.public_skills {
        repos.push((SourceId::PublicSkills, p.clone()));
    }
    let mut pinned = Vec::new();
    let mut skipped = Vec::new();
    for (id, root) in repos {
        match pin(&root) {
            Ok((remote, commit, tree)) => {
                let Some(remote) = without_credentials(&remote) else {
                    return Err(format!(
                        "Your {} repo's address contains a password or token. Remove it from the remote URL, then send again.",
                        label(&id).to_lowercase()
                    ));
                };
                pinned.push(Source {
                    id,
                    remote,
                    commit,
                    tree,
                });
            }
            Err(reason) => skipped.push(Skipped {
                source: label(&id),
                reason,
            }),
        }
    }
    let manifest = Manifest {
        format: FORMAT,
        number,
        created: now,
        hub: hub.to_owned(),
        sources: pinned,
        owner_keys,
    };
    Ok((manifest, skipped))
}

/// The remote as machines may see it. A user name alone is dropped from `http(s)` addresses
/// (it may be a token) and kept for SSH, which needs it. None when the address holds a
/// password or token (`user:secret@`), which must never go into an update.
fn without_credentials(remote: &str) -> Option<String> {
    let Some((scheme, rest)) = remote.split_once("://") else {
        // Paths and scp-style `git@host:path` carry no password.
        return Some(remote.to_owned());
    };
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, path) = rest.split_at(authority_end);
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return Some(remote.to_owned());
    };
    if userinfo.contains(':') {
        return None;
    }
    let ssh = matches!(
        scheme.to_ascii_lowercase().as_str(),
        "ssh" | "git+ssh" | "ssh+git"
    );
    if ssh {
        Some(remote.to_owned())
    } else {
        Some(format!("{scheme}://{host}{path}"))
    }
}

/// The name the UI uses for a source.
#[must_use]
pub fn label(id: &SourceId) -> String {
    match id {
        SourceId::Knowledge => "Knowledge".to_owned(),
        SourceId::PublicSkills => "Public skills".to_owned(),
        SourceId::Project(p) => p.to_string(),
    }
}

fn pin(root: &Path) -> Result<(String, CommitId, TreeHash), String> {
    let repo = atlas_git::Repo::open(root).map_err(|e| e.to_string())?;
    let remote = repo
        .remote_url()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "has no git remote, so machines can't fetch it".to_owned())?;
    let (commit, tree) = repo
        .upstream_head()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "isn't pushed anywhere yet, so machines can't fetch it".to_owned())?;
    Ok((
        remote,
        CommitId::new(commit).map_err(|e| e.to_string())?,
        TreeHash::new(tree).map_err(|e| e.to_string())?,
    ))
}

/// Signed releases, one file each, in the hub's state folder.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    #[must_use]
    pub fn new(state_dir: &Path) -> Self {
        Self {
            dir: state_dir.join("releases"),
        }
    }

    fn path(&self, number: u64) -> PathBuf {
        self.dir.join(format!("{number:08}.json"))
    }

    /// Release numbers present, ascending.
    #[must_use]
    pub fn numbers(&self) -> Vec<u64> {
        let mut out: Vec<u64> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.strip_suffix(".json")?.parse().ok())
            .collect();
        out.sort_unstable();
        out
    }

    #[must_use]
    pub fn latest(&self) -> Option<SignedRelease> {
        self.numbers().last().and_then(|n| self.get(*n))
    }

    #[must_use]
    pub fn get(&self, number: u64) -> Option<SignedRelease> {
        let bytes = std::fs::read(self.path(number)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    /// Saves a signed release. Refuses to replace one: a number is used once.
    ///
    /// # Errors
    /// If the number is taken or the file can't be written.
    pub fn save(&self, release: &SignedRelease) -> Result<(), String> {
        use std::io::Write as _;
        use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec_pretty(release).map_err(|e| e.to_string())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(self.path(release.number))
            .map_err(|e| format!("release {} can't be saved: {e}", release.number))?;
        f.write_all(&bytes)
            .and_then(|()| f.sync_all())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atlas_core::release::Signature;

    #[test]
    fn a_release_number_is_used_once() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path());
        let release = |n| SignedRelease {
            number: n,
            manifest: "e30".into(),
            signature: Signature {
                credential_id: "c".into(),
                authenticator_data: "a".into(),
                client_data_json: "j".into(),
                signature: "s".into(),
            },
        };
        assert!(store.latest().is_none());
        store.save(&release(1)).unwrap();
        store.save(&release(2)).unwrap();
        assert!(store.save(&release(2)).is_err());
        assert_eq!(store.numbers(), vec![1, 2]);
        assert_eq!(store.latest().unwrap().number, 2);
    }

    #[test]
    fn remotes_go_out_without_credentials() {
        assert_eq!(
            without_credentials("https://user:token@host/repo.git"),
            None
        );
        assert_eq!(without_credentials("ssh://git:secret@host/repo.git"), None);
        assert_eq!(
            without_credentials("https://token@host/repo.git").as_deref(),
            Some("https://host/repo.git")
        );
        for kept in [
            "https://host/repo.git",
            "ssh://git@host/repo.git",
            "git@github.com:me/repo.git",
            "/srv/git/repo.git",
            "https://host/a@b",
        ] {
            assert_eq!(without_credentials(kept).as_deref(), Some(kept));
        }
    }

    #[test]
    fn a_remote_with_a_password_refuses_to_send() {
        let tmp = tempfile::tempdir().unwrap();
        let k = tmp.path().join("k");
        std::fs::create_dir_all(&k).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(&k)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "T"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
        git(&[
            "remote",
            "add",
            "origin",
            "https://user:token@host/repo.git",
        ]);
        // A remote-tracking ref, as after a push, so the commit counts as pushed.
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        git(&["branch", "-q", "--set-upstream-to=origin/main"]);
        let sources = Sources {
            home: tmp.path().into(),
            knowledge: k.clone(),
            public_skills: None,
            projects: Vec::new(),
        };
        let refused = build(&sources, 1, "https://hub", Vec::new(), 0).unwrap_err();
        assert_eq!(
            refused,
            "Your knowledge repo's address contains a password or token. Remove it from the remote URL, then send again."
        );
    }

    #[test]
    fn only_what_machines_use_is_pinned_at_its_upstream() {
        let tmp = tempfile::tempdir().unwrap();
        let git = |dir: &Path, args: &[&str]| -> String {
            let out = std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}");
            String::from_utf8_lossy(&out.stdout).trim().to_owned()
        };
        let init = |name: &str| {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", "main"]);
            git(&dir, &["config", "user.name", "T"]);
            git(&dir, &["config", "user.email", "t@example.com"]);
            git(&dir, &["commit", "-q", "--allow-empty", "-m", "start"]);
            dir
        };
        let k = init("k");
        let shop = init("shop");
        let sources = Sources {
            home: tmp.path().into(),
            knowledge: k.clone(),
            public_skills: None,
            projects: vec![atlas_fs::Project {
                name: atlas_core::kernel::Slug::new("shop").unwrap(),
                root: shop,
            }],
        };
        let (m, skipped) = build(&sources, 1, "https://hub", Vec::new(), 0).unwrap();
        assert_eq!(m.sources, Vec::new(), "projects never go in");
        assert_eq!(skipped.len(), 1);
        assert!(skipped[0].reason.contains("no git remote"));

        let bare = tmp.path().join("k.git");
        git(tmp.path(), &["init", "-q", "--bare", "k.git"]);
        git(&k, &["remote", "add", "origin", bare.to_str().unwrap()]);
        let (_, skipped) = build(&sources, 1, "https://hub", Vec::new(), 0).unwrap();
        assert!(skipped[0].reason.contains("isn't pushed"), "{skipped:?}");

        git(&k, &["push", "-q", "-u", "origin", "main"]);
        let pushed = git(&k, &["rev-parse", "HEAD"]);
        git(&k, &["commit", "-q", "--allow-empty", "-m", "local only"]);
        let (m, skipped) = build(&sources, 1, "https://hub", Vec::new(), 0).unwrap();
        assert_eq!(skipped, Vec::new());
        assert_eq!(m.sources.len(), 1);
        assert_eq!(m.sources[0].id, SourceId::Knowledge);
        assert_eq!(m.sources[0].remote, bare.to_str().unwrap());
        assert_eq!(
            m.sources[0].commit.as_str(),
            pushed,
            "the upstream's commit"
        );
    }
}
