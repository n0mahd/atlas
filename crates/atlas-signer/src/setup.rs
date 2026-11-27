//! One-time setup links: how a passkey is added without signing in.
//!
//! `atlas-hub setup` runs on the hub machine, so whoever runs it already controls the hub.
//! It issues a random token; the link carries the token and the state folder keeps only
//! its SHA-256 and expiry. A link works once and for a few minutes. It also recovers
//! access when every passkey is lost.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use webauthn_rs::prelude::Uuid;

use crate::SignerError;

const FILE: &str = "setup-links.json";

#[derive(Default, Serialize, Deserialize)]
struct Links {
    links: Vec<Link>,
}

#[derive(Serialize, Deserialize)]
struct Link {
    /// SHA-256 of the token, hex.
    hash: String,
    /// Seconds since the Unix epoch.
    expires: u64,
}

fn hash(token: &str) -> String {
    Sha256::digest(token.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

fn load(state_dir: &Path) -> Links {
    fs::read(path(state_dir))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store(state_dir: &Path, links: &Links) -> Result<(), SignerError> {
    let err = |e: std::io::Error| SignerError::Store(e.to_string());
    let bytes = serde_json::to_vec_pretty(links).map_err(|e| SignerError::Store(e.to_string()))?;
    let tmp = state_dir.join(format!("{FILE}.tmp"));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(err)?;
    f.write_all(&bytes).map_err(err)?;
    f.sync_all().map_err(err)?;
    fs::rename(&tmp, path(state_dir)).map_err(err)
}

/// Issues a token valid for `ttl`. Older expired links are dropped.
///
/// # Errors
/// If the state folder can't be written.
pub fn issue(state_dir: &Path, ttl: Duration) -> Result<String, SignerError> {
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_dir)
        .map_err(|e| SignerError::Store(e.to_string()))?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let now = crate::now();
    let mut links = load(state_dir);
    links.links.retain(|l| l.expires > now);
    links.links.push(Link {
        hash: hash(&token),
        expires: now + ttl.as_secs(),
    });
    store(state_dir, &links)?;
    Ok(token)
}

fn plausible(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a token is live, without using it up.
#[must_use]
pub fn is_valid(state_dir: &Path, token: &str) -> bool {
    plausible(token) && {
        let h = hash(token);
        let now = crate::now();
        load(state_dir)
            .links
            .iter()
            .any(|l| l.hash == h && l.expires > now)
    }
}

/// Uses a token up. True only the first time, and only before it expires.
///
/// # Errors
/// If the state folder can't be written.
pub fn redeem(state_dir: &Path, token: &str) -> Result<bool, SignerError> {
    if !plausible(token) {
        return Ok(false);
    }
    let h = hash(token);
    let now = crate::now();
    let mut links = load(state_dir);
    let before = links.links.len();
    let live = links.links.iter().any(|l| l.hash == h && l.expires > now);
    links.links.retain(|l| l.hash != h && l.expires > now);
    if links.links.len() != before {
        store(state_dir, &links)?;
    }
    Ok(live)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_link_works_once() {
        let tmp = tempfile::tempdir().unwrap();
        let token = issue(tmp.path(), Duration::from_mins(15)).unwrap();
        assert!(is_valid(tmp.path(), &token));
        assert!(is_valid(tmp.path(), &token), "checking doesn't use it up");
        assert!(redeem(tmp.path(), &token).unwrap());
        assert!(!redeem(tmp.path(), &token).unwrap());
        assert!(!is_valid(tmp.path(), &token));
    }

    #[test]
    fn only_the_hash_is_stored_privately() {
        let tmp = tempfile::tempdir().unwrap();
        let token = issue(tmp.path(), Duration::from_mins(15)).unwrap();
        let text = fs::read_to_string(tmp.path().join(FILE)).unwrap();
        assert!(!text.contains(&token));
        assert_eq!(
            fs::metadata(tmp.path().join(FILE))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    #[test]
    fn expired_and_bogus_links_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let token = issue(tmp.path(), Duration::ZERO).unwrap();
        assert!(!is_valid(tmp.path(), &token));
        assert!(!redeem(tmp.path(), &token).unwrap());
        assert!(!is_valid(tmp.path(), "../../etc/passwd"));
        assert!(!is_valid(tmp.path(), &"a".repeat(64)));
    }
}
