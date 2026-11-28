//! The Release context: numbered snapshots of what goes out to machines (decision 0007).
//!
//! The hub prepares a [`Manifest`]; the owner signs its digest with a passkey; each
//! machine's client checks the signature, that the number only goes up, and that what it
//! fetched from git matches the pinned trees. The hub can't sign, so a compromised hub
//! can't push changes. Pure types and rules here; signing and checking live in adapters.

use serde::{Deserialize, Serialize};

use crate::kernel::{Slug, TreeHash};

/// The manifest format. Bumped when a field changes meaning; clients refuse newer formats.
/// Adding a field old clients can safely ignore (such as `projects`) doesn't bump it: a
/// bump would make every older client refuse every update until it is upgraded.
pub const FORMAT: u32 = 1;

/// Which store repo a source is.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "name")]
pub enum SourceId {
    Knowledge,
    PublicSkills,
    Project(Slug),
}

/// A git object id (commit): 40 or 64 lowercase hex digits.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CommitId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid commit id {0:?}")]
pub struct InvalidCommitId(String);

impl CommitId {
    /// # Errors
    /// Unless the value is 40 or 64 lowercase hex digits.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidCommitId> {
        let value = value.into();
        let hex = value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if hex && matches!(value.len(), 40 | 64) {
            Ok(Self(value))
        } else {
            Err(InvalidCommitId(value))
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for CommitId {
    type Error = InvalidCommitId;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<CommitId> for String {
    fn from(c: CommitId) -> Self {
        c.0
    }
}

/// One repo pinned by a release: where to fetch it and exactly what must arrive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,
    /// A git remote the client can fetch from. Untrusted: the tree hash is what counts.
    pub remote: String,
    pub commit: CommitId,
    pub tree: TreeHash,
}

/// An owner passkey's public key, so clients can check later releases. A release can add
/// or drop keys; the client only trusts a new set once a release signed by a key it
/// already trusts carries it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerKey {
    /// Credential id, base64url without padding.
    pub credential_id: String,
    /// `es256` or `eddsa`.
    pub alg: String,
    /// The raw public key, base64url without padding.
    pub public_key: String,
    /// The owner's name for it, such as "Laptop".
    pub name: String,
}

/// A registered project, so each machine can find its own checkouts of it (decision
/// 0027). Not pinned: a project's content is the project's (decision 0003), and its
/// remote only identifies it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ProjectRef {
    pub name: Slug,
    /// The project's git remote, without any credentials ([`without_credentials`]).
    pub remote: String,
}

/// What a release says. Serialised once by the hub; the exact bytes are what get signed
/// and shipped, so nobody re-serialises it before checking the digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub number: u64,
    /// Seconds since the Unix epoch.
    pub created: u64,
    /// The hub's origin; passkey signatures are checked against it.
    pub hub: String,
    pub sources: Vec<Source>,
    pub owner_keys: Vec<OwnerKey>,
    /// Registered projects, by name. Last, and left out when empty, so a manifest without
    /// projects has the same bytes as before the field existed; older clients ignore it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ProjectRef>,
}

/// The remote as machines may see it in an update. A user name alone is dropped from
/// `http(s)` addresses (it may be a token) and kept for SSH, which needs it. None when the
/// address holds a password or token (`user:secret@`), which must never go into an update.
/// Paths and scp-style `git@host:path` carry no password and are returned as they are.
#[must_use]
pub fn without_credentials(remote: &str) -> Option<String> {
    let Some((scheme, rest)) = remote.split_once("://") else {
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

/// What every way of writing one repo's remote has in common, to tell whether two
/// remotes are the same repo: `git@host:o/r.git`, `ssh://git@host/o/r`,
/// `https://host/o/r/` all give `host/o/r`; a local path gives itself. The host is
/// compared without case, user and port; the path without a `.git` suffix or trailing
/// slash.
#[must_use]
pub fn remote_key(remote: &str) -> String {
    let r = remote.trim();
    let schemes = [
        "ssh://",
        "git+ssh://",
        "ssh+git://",
        "git://",
        "https://",
        "http://",
    ];
    let (host, path) = if let Some(rest) = schemes.iter().find_map(|s| r.strip_prefix(s)) {
        let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
        (Some(authority), path)
    } else if let Some(rest) = r.strip_prefix("file://") {
        (None, rest)
    } else {
        match r.split_once(':') {
            // scp-like `[user@]host:path`, unless the part before ':' looks like a path.
            Some((authority, path)) if !authority.contains('/') && !authority.is_empty() => {
                (Some(authority), path)
            }
            _ => (None, r),
        }
    };
    let mut path = path.trim_end_matches('/');
    path = path
        .strip_suffix(".git")
        .unwrap_or(path)
        .trim_end_matches('/');
    match host {
        Some(authority) => {
            let host = authority.rsplit('@').next().unwrap_or(authority);
            let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
            format!("{host}/{}", path.trim_start_matches('/'))
        }
        None => path.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseRejected {
    #[error("release format {0} is newer than this client understands")]
    NewerFormat(u32),
    #[error("release {got} isn't newer than release {applied}, which is already applied")]
    NotNewer { applied: u64, got: u64 },
    #[error("release lists no owner keys, so nothing after it could be trusted")]
    NoKeys,
    #[error("release lists the same source twice")]
    DuplicateSource,
    #[error("release was signed for hub {got}, not {expected}")]
    WrongHub { expected: String, got: String },
}

impl Manifest {
    /// The rules a client applies after the signature checks out: a format it knows,
    /// the hub it paired with, a number above the last one it applied (no replays or
    /// rollbacks), at least one key, and each source once.
    ///
    /// # Errors
    /// The first rule the release breaks.
    pub fn check(&self, hub: &str, last_applied: Option<u64>) -> Result<(), ReleaseRejected> {
        if self.format > FORMAT {
            return Err(ReleaseRejected::NewerFormat(self.format));
        }
        if self.hub != hub {
            return Err(ReleaseRejected::WrongHub {
                expected: hub.to_owned(),
                got: self.hub.clone(),
            });
        }
        if let Some(applied) = last_applied
            && self.number <= applied
        {
            return Err(ReleaseRejected::NotNewer {
                applied,
                got: self.number,
            });
        }
        if self.owner_keys.is_empty() {
            return Err(ReleaseRejected::NoKeys);
        }
        let mut ids: Vec<&SourceId> = self.sources.iter().map(|s| &s.id).collect();
        ids.sort();
        if ids.windows(2).any(|w| w[0] == w[1]) {
            return Err(ReleaseRejected::DuplicateSource);
        }
        Ok(())
    }

    #[must_use]
    pub fn source(&self, id: &SourceId) -> Option<&Source> {
        self.sources.iter().find(|s| &s.id == id)
    }
}

/// A release as stored and shipped: the manifest's exact bytes and the passkey assertion
/// over their digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignedRelease {
    pub number: u64,
    /// The manifest's JSON, base64url without padding: these bytes are what was signed.
    pub manifest: String,
    pub signature: Signature,
}

/// A WebAuthn assertion, fields base64url without padding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Signature {
    pub credential_id: String,
    pub authenticator_data: String,
    pub client_data_json: String,
    pub signature: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(number: u64) -> Manifest {
        Manifest {
            format: FORMAT,
            number,
            created: 0,
            hub: "https://hub.example".into(),
            sources: vec![Source {
                id: SourceId::Knowledge,
                remote: "https://git.example/k.git".into(),
                commit: CommitId::new("a".repeat(40)).unwrap(),
                tree: TreeHash::new("b".repeat(40)).unwrap(),
            }],
            owner_keys: vec![OwnerKey {
                credential_id: "id".into(),
                alg: "es256".into(),
                public_key: "pk".into(),
                name: "Laptop".into(),
            }],
            projects: Vec::new(),
        }
    }

    /// The manifest as a client from before `projects` knew it.
    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct OldManifest {
        format: u32,
        number: u64,
        created: u64,
        hub: String,
        sources: Vec<Source>,
        owner_keys: Vec<OwnerKey>,
    }

    /// A manifest as a client from before `projects` serialised it.
    const OLD: &str = r#"{"format":1,"number":3,"created":0,"hub":"https://hub.example","sources":[{"id":{"kind":"knowledge"},"remote":"https://git.example/k.git","commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","tree":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}],"owner_keys":[{"credential_id":"id","alg":"es256","public_key":"pk","name":"Laptop"}]}"#;

    #[test]
    fn manifests_without_projects_keep_their_bytes() {
        let old: Manifest = serde_json::from_str(OLD).unwrap();
        assert_eq!(old.projects, Vec::new());
        assert_eq!(old, manifest(3));
        assert_eq!(
            serde_json::to_string(&old).unwrap(),
            OLD,
            "same bytes as before"
        );

        let mut m = manifest(4);
        m.projects.push(ProjectRef {
            name: Slug::new("shop").unwrap(),
            remote: "git@git.example:o/shop.git".into(),
        });
        let bytes = serde_json::to_vec(&m).unwrap();
        assert_eq!(bytes, serde_json::to_vec(&m.clone()).unwrap());
        let text = String::from_utf8(bytes).unwrap();
        assert!(
            text.ends_with(
                r#""projects":[{"name":"shop","remote":"git@git.example:o/shop.git"}]}"#
            ),
            "{text}"
        );
        // An older client reads the same bytes and ignores the field.
        let old: OldManifest = serde_json::from_str(&text).unwrap();
        assert_eq!((old.format, old.number), (FORMAT, 4));
        assert_eq!(old.sources, m.sources);
    }

    #[test]
    fn remotes_of_one_repo_share_a_key() {
        let key = "github.com/owner/repo";
        for r in [
            "git@github.com:owner/repo.git",
            "git@github.com:owner/repo",
            "ssh://git@github.com/owner/repo.git",
            "ssh://git@github.com:22/owner/repo/",
            "https://github.com/owner/repo",
            "https://GitHub.com/owner/repo.git/",
            "https://user:token@github.com/owner/repo.git",
            "git://github.com/owner/repo",
        ] {
            assert_eq!(remote_key(r), key, "{r}");
        }
        assert_eq!(remote_key("/srv/git/repo.git"), "/srv/git/repo");
        assert_eq!(remote_key("file:///srv/git/repo/"), "/srv/git/repo");
        assert_ne!(remote_key("git@github.com:owner/other"), key);
        assert_ne!(remote_key("git@gitlab.com:owner/repo"), key);
    }

    #[test]
    fn credentials_never_go_out() {
        assert_eq!(
            without_credentials("https://user:ghp_secret@github.com/o/r.git"),
            None,
            "a password never goes out, nor does a stripped copy that would hide it"
        );
        assert_eq!(
            without_credentials("http://token@host.example/o/r").as_deref(),
            Some("http://host.example/o/r")
        );
        for kept in ["git@github.com:o/r.git", "https://github.com/o/r", "/srv/r"] {
            assert_eq!(without_credentials(kept).as_deref(), Some(kept));
        }
    }

    #[test]
    fn numbers_only_go_up() {
        let m = manifest(5);
        assert!(m.check("https://hub.example", None).is_ok());
        assert!(m.check("https://hub.example", Some(4)).is_ok());
        assert_eq!(
            m.check("https://hub.example", Some(5)),
            Err(ReleaseRejected::NotNewer { applied: 5, got: 5 })
        );
        assert!(
            m.check("https://hub.example", Some(9)).is_err(),
            "no rollback"
        );
    }

    #[test]
    fn other_rules() {
        let mut m = manifest(1);
        assert!(matches!(
            m.check("https://other", None),
            Err(ReleaseRejected::WrongHub { .. })
        ));
        m.format = FORMAT + 1;
        assert_eq!(
            m.check("https://hub.example", None),
            Err(ReleaseRejected::NewerFormat(FORMAT + 1))
        );
        let mut m = manifest(1);
        m.owner_keys.clear();
        assert_eq!(
            m.check("https://hub.example", None),
            Err(ReleaseRejected::NoKeys)
        );
        let mut m = manifest(1);
        m.sources.push(m.sources[0].clone());
        assert_eq!(
            m.check("https://hub.example", None),
            Err(ReleaseRejected::DuplicateSource)
        );
    }

    #[test]
    fn source_ids_serialise_plainly() {
        let p = SourceId::Project(Slug::new("shop").unwrap());
        assert_eq!(
            serde_json::to_string(&p).unwrap(),
            r#"{"kind":"project","name":"shop"}"#
        );
        assert_eq!(
            serde_json::to_string(&SourceId::Knowledge).unwrap(),
            r#"{"kind":"knowledge"}"#
        );
        assert!(CommitId::new("XYZ").is_err());
    }
}
