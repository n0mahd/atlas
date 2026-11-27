//! Releases on the hub: build a manifest from the repos as they stand, keep signed
//! releases, and hand the latest to paired clients. The hub never signs (decision 0007).

use std::path::{Path, PathBuf};

use atlas_core::kernel::TreeHash;
use atlas_core::release::{CommitId, FORMAT, Manifest, OwnerKey, SignedRelease, Source, SourceId};
use atlas_fs::Sources;

/// A repo left out of a release, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    pub source: String,
    pub reason: String,
}

/// Builds the next manifest from each store repo's current commit.
#[must_use]
pub fn build(
    sources: &Sources,
    number: u64,
    hub: &str,
    owner_keys: Vec<OwnerKey>,
    now: u64,
) -> (Manifest, Vec<Skipped>) {
    let mut repos: Vec<(SourceId, PathBuf)> =
        vec![(SourceId::Knowledge, sources.knowledge.clone())];
    if let Some(p) = &sources.public_skills {
        repos.push((SourceId::PublicSkills, p.clone()));
    }
    repos.extend(
        sources
            .projects
            .iter()
            .map(|p| (SourceId::Project(p.name.clone()), p.root.clone())),
    );

    let mut pinned = Vec::new();
    let mut skipped = Vec::new();
    for (id, root) in repos {
        let label = label(&id);
        match pin(&root) {
            Ok((remote, commit, tree)) => pinned.push(Source {
                id,
                remote,
                commit,
                tree,
            }),
            Err(reason) => skipped.push(Skipped {
                source: label,
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
    (manifest, skipped)
}

fn label(id: &SourceId) -> String {
    match id {
        SourceId::Knowledge => "knowledge".to_owned(),
        SourceId::PublicSkills => "public skills".to_owned(),
        SourceId::Project(p) => p.to_string(),
    }
}

fn pin(root: &Path) -> Result<(String, CommitId, TreeHash), String> {
    let repo = atlas_git::Repo::open(root).map_err(|e| e.to_string())?;
    let remote = repo
        .remote_url()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "has no git remote, so machines couldn't fetch it".to_owned())?;
    let (commit, tree) = repo.head().map_err(|e| e.to_string())?;
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
    fn repos_without_a_remote_are_skipped_with_a_reason() {
        let tmp = tempfile::tempdir().unwrap();
        let k = tmp.path().join("k");
        std::fs::create_dir_all(&k).unwrap();
        let git = |args: &[&str]| {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&k)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "T"]);
        git(&["config", "user.email", "t@example.com"]);
        git(&["commit", "-q", "--allow-empty", "-m", "start"]);
        let sources = Sources {
            home: tmp.path().into(),
            knowledge: k.clone(),
            public_skills: None,
            projects: Vec::new(),
        };
        let (m, skipped) = build(&sources, 1, "https://hub", Vec::new(), 0);
        assert_eq!(m.sources, Vec::new());
        assert_eq!(skipped[0].source, "knowledge");
        git(&["remote", "add", "origin", "https://git.example/k.git"]);
        let (m, skipped) = build(&sources, 1, "https://hub", Vec::new(), 0);
        assert_eq!(skipped, Vec::new());
        assert_eq!(m.sources[0].remote, "https://git.example/k.git");
    }
}
