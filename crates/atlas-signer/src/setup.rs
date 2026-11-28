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

/// Setup links: add a passkey without signing in.
pub const SETUP_LINKS: &str = "setup-links.json";
/// Pairing codes: let a machine's client pair with the hub.
pub const PAIRING_CODES: &str = "pairing-codes.json";

#[derive(Default, Serialize, Deserialize)]
struct CodeFile {
    links: Vec<Link>,
    /// When recent wrong pairing codes were tried, seconds since the Unix epoch.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    failures: Vec<u64>,
    /// Wrong pairing codes tried since the last one was issued.
    #[serde(default, skip_serializing_if = "is_zero")]
    failed_since_issue: u32,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if"
)]
const fn is_zero(n: &u32) -> bool {
    *n == 0
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

/// Holds a code file for one load-check-store: a mutex for this process's threads and an
/// exclusive file lock for other processes (the hub and `atlas-hub` commands share the
/// state folder). Dropped, both are released.
struct Locked {
    _file: fs::File,
    _guard: std::sync::MutexGuard<'static, ()>,
}

fn lock(state_dir: &Path, file: &str) -> Result<Locked, SignerError> {
    static IN_PROCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let err = |e: std::io::Error| SignerError::Store(e.to_string());
    let guard = IN_PROCESS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(state_dir)
        .map_err(err)?;
    let f = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(state_dir.join(format!("{file}.lock")))
        .map_err(err)?;
    f.lock().map_err(err)?;
    Ok(Locked {
        _file: f,
        _guard: guard,
    })
}

fn path(state_dir: &Path, file: &str) -> PathBuf {
    state_dir.join(file)
}

fn load(state_dir: &Path, file: &str) -> CodeFile {
    fs::read(path(state_dir, file))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn store(state_dir: &Path, file: &str, links: &CodeFile) -> Result<(), SignerError> {
    let err = |e: std::io::Error| SignerError::Store(e.to_string());
    let bytes = serde_json::to_vec_pretty(links).map_err(|e| SignerError::Store(e.to_string()))?;
    let tmp = state_dir.join(format!("{file}.tmp"));
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&tmp)
        .map_err(err)?;
    f.write_all(&bytes).map_err(err)?;
    f.sync_all().map_err(err)?;
    fs::rename(&tmp, path(state_dir, file)).map_err(err)
}

/// Issues a token valid for `ttl`. Older expired links are dropped.
///
/// # Errors
/// If the state folder can't be written.
pub fn issue(state_dir: &Path, ttl: Duration) -> Result<String, SignerError> {
    let file = SETUP_LINKS;
    let _lock = lock(state_dir, file)?;
    let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    let now = crate::now();
    let mut links = load(state_dir, file);
    links.links.retain(|l| l.expires > now);
    links.links.push(Link {
        hash: hash(&token),
        expires: now + ttl.as_secs(),
    });
    store(state_dir, file, &links)?;
    Ok(token)
}

fn plausible(token: &str) -> bool {
    token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether a token is live, without using it up.
#[must_use]
pub fn is_valid(state_dir: &Path, token: &str) -> bool {
    let file = SETUP_LINKS;
    plausible(token) && {
        let h = hash(token);
        let now = crate::now();
        load(state_dir, file)
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
    let file = SETUP_LINKS;
    if !plausible(token) {
        return Ok(false);
    }
    let h = hash(token);
    let _lock = lock(state_dir, file)?;
    let now = crate::now();
    let mut links = load(state_dir, file);
    let before = links.links.len();
    let live = links.links.iter().any(|l| l.hash == h && l.expires > now);
    links.links.retain(|l| l.hash != h && l.expires > now);
    if links.links.len() != before {
        store(state_dir, file, &links)?;
    }
    Ok(live)
}

// ---------------------------------------------------------------- pairing codes

/// Wrong pairing codes the hub takes in a rolling minute; after that every redeem is
/// refused until the minute has passed, even with a right code.
pub const PAIRING_FAILURES_PER_MINUTE: usize = 5;
/// Wrong pairing codes since the last one was issued after which every outstanding
/// pairing code is void.
pub const PAIRING_VOID_AFTER: u32 = 20;

/// Crockford base32: no I, L, O or U, so a code reads back the way it was written.
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const PAIRING_CHARS: usize = 10;

/// What redeeming a pairing code did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pairing {
    /// The code was live and is now used up.
    Paired,
    /// Not a live code. Counted against the limit.
    Wrong,
    /// Too many wrong codes lately; nothing was checked.
    TooMany,
}

/// A code as typed, in canonical form: upper case, no hyphens or spaces, and look-alikes
/// read as digits (O as 0; I and L as 1). None if it can't be a pairing code.
fn normalise(code: &str) -> Option<String> {
    let out: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .map(|c| match c.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            c => c,
        })
        .collect();
    (out.len() == PAIRING_CHARS && out.bytes().all(|b| CROCKFORD.contains(&b))).then_some(out)
}

/// Issues a pairing code valid for `ttl`: 10 Crockford base32 characters (50 random
/// bits), grouped for reading as `K7QF-9XMR2D`. Only its hash is stored.
///
/// # Errors
/// If the state folder can't be written.
pub fn issue_pairing(state_dir: &Path, ttl: Duration) -> Result<String, SignerError> {
    let _lock = lock(state_dir, PAIRING_CODES)?;
    let random = Uuid::new_v4();
    let b = random.as_bytes();
    // A v4 UUID's byte 6 carries its version; the other bytes here are random.
    let bits = [b[0], b[1], b[2], b[3], b[4], b[5], b[7]]
        .iter()
        .fold(0u64, |acc, x| acc << 8 | u64::from(*x));
    let code: String = (0..PAIRING_CHARS)
        .map(|i| CROCKFORD[usize::try_from(bits >> (5 * i) & 31).unwrap_or(0)] as char)
        .collect();
    let now = crate::now();
    let mut links = load(state_dir, PAIRING_CODES);
    links.links.retain(|l| l.expires > now);
    links.failed_since_issue = 0;
    links.links.push(Link {
        hash: hash(&code),
        expires: now + ttl.as_secs(),
    });
    store(state_dir, PAIRING_CODES, &links)?;
    Ok(format!("{}-{}", &code[..4], &code[4..]))
}

/// Uses a pairing code up, under the hub-wide limit on wrong codes.
///
/// # Errors
/// If the state folder can't be written.
pub fn redeem_pairing(state_dir: &Path, code: &str) -> Result<Pairing, SignerError> {
    let _lock = lock(state_dir, PAIRING_CODES)?;
    let now = crate::now();
    let mut links = load(state_dir, PAIRING_CODES);
    links.failures.retain(|t| t + 60 > now);
    links.links.retain(|l| l.expires > now);
    let h = normalise(code).map(|c| hash(&c));
    let live = h
        .as_ref()
        .is_some_and(|h| links.links.iter().any(|l| &l.hash == h));
    let limited = links.failures.len() >= PAIRING_FAILURES_PER_MINUTE;
    let outcome = if live && !limited {
        links.links.retain(|l| Some(&l.hash) != h.as_ref());
        Pairing::Paired
    } else {
        if !live {
            links.failures.push(now);
            links.failed_since_issue = links.failed_since_issue.saturating_add(1);
            if links.failed_since_issue >= PAIRING_VOID_AFTER {
                links.links.clear();
            }
        }
        if limited {
            Pairing::TooMany
        } else {
            Pairing::Wrong
        }
    };
    store(state_dir, PAIRING_CODES, &links)?;
    Ok(outcome)
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
        let text = fs::read_to_string(tmp.path().join(SETUP_LINKS)).unwrap();
        assert!(!text.contains(&token));
        assert_eq!(
            fs::metadata(tmp.path().join(SETUP_LINKS))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }

    /// Two redeems of one code at the same moment: exactly one of them gets it.
    #[test]
    fn a_code_redeemed_twice_at_once_works_once() {
        let tmp = tempfile::tempdir().unwrap();
        for _ in 0..50 {
            let token = issue(tmp.path(), Duration::from_mins(15)).unwrap();
            let start = std::sync::Barrier::new(2);
            let wins = std::thread::scope(|s| {
                let racers: Vec<_> = (0..2)
                    .map(|_| {
                        s.spawn(|| {
                            start.wait();
                            redeem(tmp.path(), &token).unwrap()
                        })
                    })
                    .collect();
                racers
                    .into_iter()
                    .map(|r| r.join().unwrap())
                    .filter(|won| *won)
                    .count()
            });
            assert_eq!(wins, 1);
        }
    }

    #[test]
    fn pairing_codes_are_grouped_crockford_and_read_back_loosely() {
        let tmp = tempfile::tempdir().unwrap();
        let code = issue_pairing(tmp.path(), Duration::from_mins(15)).unwrap();
        assert_eq!(code.len(), 11);
        assert_eq!(code.as_bytes()[4], b'-');
        assert!(
            code.bytes()
                .filter(|b| *b != b'-')
                .all(|b| CROCKFORD.contains(&b))
        );
        assert_eq!(normalise("K7QF-9XMR2"), None, "ten characters");
        assert_eq!(normalise("k7qf-9xmr2d").as_deref(), Some("K7QF9XMR2D"));
        assert_eq!(normalise("O0IL 9XMR2D").as_deref(), Some("00119XMR2D"));
        assert_eq!(normalise("UUUUUUUUUU"), None);
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
