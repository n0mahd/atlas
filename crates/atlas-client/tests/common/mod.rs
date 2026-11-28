//! Shared by the whole-pass tests: a temp home, local git repos standing in for the
//! remotes, and releases signed with a P-256 key made here, exactly as a passkey would.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use atlas_client::config::{Config, Paths, State};
use atlas_client::pass::{self, Fetched, Pass};
use atlas_core::kernel::TreeHash;
use atlas_core::release::{
    CommitId, FORMAT, Manifest, OwnerKey, Signature, SignedRelease, Source, SourceId,
};
use atlas_verify::encode;
use p256::ecdsa::SigningKey;
use sha2::{Digest as _, Sha256};

pub const HUB: &str = "https://hub.example";
pub const RP: &str = "hub.example";

pub struct Key {
    pub id: Vec<u8>,
    pub signing: SigningKey,
    pub name: &'static str,
}

impl Key {
    pub fn new(seed: u8, name: &'static str) -> Self {
        Key {
            id: vec![seed; 16],
            signing: SigningKey::from_slice(&Sha256::digest([b'k', seed])).unwrap(),
            name,
        }
    }

    pub fn owner(&self) -> OwnerKey {
        OwnerKey {
            credential_id: encode(&self.id),
            alg: "es256".into(),
            public_key: encode(self.signing.verifying_key().to_sec1_point(false).as_bytes()),
            name: self.name.into(),
        }
    }

    /// A WebAuthn assertion over `challenge`, as a browser and authenticator make it.
    pub fn sign(&self, challenge: &[u8]) -> Signature {
        use p256::ecdsa::signature::Signer as _;
        let mut auth = Sha256::digest(RP.as_bytes()).to_vec();
        auth.push(0x05); // user present + verified
        auth.extend_from_slice(&1u32.to_be_bytes());
        let client = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"{HUB}"}}"#,
            encode(challenge)
        );
        let mut message = auth.clone();
        message.extend_from_slice(&Sha256::digest(client.as_bytes()));
        let sig: p256::ecdsa::Signature = self.signing.sign(&message);
        Signature {
            credential_id: encode(&self.id),
            authenticator_data: encode(&auth),
            client_data_json: encode(client.as_bytes()),
            signature: encode(sig.to_der().as_bytes()),
        }
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
        .args([
            "-c",
            "init.defaultBranch=main",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

pub fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// The release source for `dir` at its HEAD.
pub fn source_at(dir: &Path, id: SourceId) -> Source {
    Source {
        id,
        remote: dir.to_string_lossy().into_owned(),
        commit: CommitId::new(git(dir, &["rev-parse", "HEAD"])).unwrap(),
        tree: TreeHash::new(git(dir, &["rev-parse", "HEAD^{tree}"])).unwrap(),
    }
}

/// A repo made of `files`, committed; returns it as a release source.
pub fn repo(dir: &Path, id: SourceId, files: &[(&str, &str)]) -> Source {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "--quiet"]);
    for (path, text) in files {
        write(&dir.join(path), text);
    }
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "--quiet", "-m", "store"]);
    source_at(dir, id)
}

pub struct World {
    pub _tmp: tempfile::TempDir,
    /// The temp folder everything lives in (canonical).
    pub root: PathBuf,
    pub paths: Paths,
    pub config: Config,
    pub state: State,
    pub sources: Vec<Source>,
    pub adopt: PathBuf,
}

pub const HOOK: &str =
    "description = \"Guard\"\nevent = \"pre-tool-use\"\nmatcher = \"Bash\"\nrun = \"run.sh\"\n";

impl World {
    pub fn new(key: &Key) -> Self {
        Self::with(key, &[])
    }

    /// A world whose knowledge repo also holds `extra` files.
    pub fn with(key: &Key, extra: &[(&str, &str)]) -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let home = root.join("home");
        for marker in [".claude", ".codex"] {
            fs::create_dir_all(home.join(marker)).unwrap();
        }
        let mut files = vec![
            ("AGENTS.md", "# Global\n"),
            ("skills/alpha/SKILL.md", "---\nname: alpha\n---\n"),
            ("skills/gamma/SKILL.md", "---\nname: gamma\n---\n"),
            ("hooks/guard/hook.toml", HOOK),
            ("hooks/guard/run.sh", "#!/bin/sh\nexit 0\n"),
        ];
        files.extend_from_slice(extra);
        let knowledge = repo(&root.join("remotes/knowledge"), SourceId::Knowledge, &files);
        let public = repo(
            &root.join("remotes/public"),
            SourceId::PublicSkills,
            &[
                ("beta/SKILL.md", "---\nname: beta\n---\n"),
                ("alpha/SKILL.md", "---\nname: alpha\n---\n"),
            ],
        );
        let adopt = root.join("old-skills");
        fs::create_dir_all(adopt.join("alpha")).unwrap();
        World {
            _tmp: tmp,
            root,
            paths: Paths::new(&home),
            config: Config {
                hub: HUB.into(),
                client_id: "c1".into(),
                token: "t".into(),
                machine: "box".into(),
                adopt: vec![adopt.to_string_lossy().into_owned()],
            },
            state: State {
                owner_keys: vec![key.owner()],
                ..State::default()
            },
            sources: vec![knowledge, public],
            adopt,
        }
    }

    pub fn home(&self) -> &Path {
        &self.paths.home
    }

    pub fn copy(&self, rel: &str) -> PathBuf {
        self.paths.sources.join(rel)
    }

    /// The knowledge repo standing in for its remote.
    pub fn knowledge_remote(&self) -> PathBuf {
        self.root.join("remotes/knowledge")
    }

    /// Changes the knowledge repo (None removes a file) and pins the new commit.
    pub fn change_knowledge(&mut self, files: &[(&str, Option<&str>)]) {
        let dir = self.knowledge_remote();
        for (path, text) in files {
            match text {
                Some(t) => write(&dir.join(path), t),
                None => fs::remove_file(dir.join(path)).unwrap(),
            }
        }
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "--quiet", "-m", "change"]);
        self.sources[0] = source_at(&dir, SourceId::Knowledge);
    }

    pub fn manifest(&self, number: u64, keys: &[&Key]) -> Manifest {
        Manifest {
            format: FORMAT,
            number,
            created: 0,
            hub: HUB.into(),
            sources: self.sources.clone(),
            owner_keys: keys.iter().map(|k| k.owner()).collect(),
        }
    }

    /// Runs a pass and keeps the resulting state, as `atlasd run` does.
    pub fn run(&mut self, release: &SignedRelease, dry_run: bool) -> Pass {
        let pass = pass::run(
            &self.paths,
            &self.config,
            &self.state,
            Fetched::Release(Box::new(release.clone())),
            dry_run,
            1,
        );
        self.state = pass.state.clone();
        pass
    }

    /// Signs and runs release `number` as it stands, and checks it applied.
    pub fn apply(&mut self, number: u64, key: &Key) -> Pass {
        let release = signed(&self.manifest(number, &[key]), key);
        let pass = self.run(&release, false);
        assert!(!pass.failed, "{:?}", pass.problems);
        pass
    }
}

pub fn signed(manifest: &Manifest, by: &Key) -> SignedRelease {
    let bytes = serde_json::to_vec(manifest).unwrap();
    SignedRelease {
        number: manifest.number,
        manifest: encode(&bytes),
        signature: by.sign(&Sha256::digest(&bytes)),
    }
}

/// Every entry under `dir`, without following links: link targets, file bytes and modes.
pub fn snapshot(dir: &Path) -> BTreeMap<PathBuf, String> {
    fn walk(dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
        for e in fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            let m = fs::symlink_metadata(&p).unwrap();
            let v = if m.file_type().is_symlink() {
                format!("link {}", fs::read_link(&p).unwrap().display())
            } else if m.is_dir() {
                walk(&p, out);
                "dir".to_owned()
            } else {
                format!(
                    "file {:o} {}",
                    m.permissions().mode(),
                    String::from_utf8_lossy(&fs::read(&p).unwrap())
                )
            };
            out.insert(p, v);
        }
    }
    let mut out = BTreeMap::new();
    walk(dir, &mut out);
    out
}

/// The home, minus Atlas's own cache of local copies.
pub fn harness_files(w: &World) -> BTreeMap<PathBuf, String> {
    let mut s = snapshot(w.home());
    s.retain(|p, _| !p.starts_with(&w.paths.sources));
    s
}

pub fn link(path: &Path) -> PathBuf {
    fs::read_link(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Whether `path` is a link (or anything at all), without following it.
pub fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}
