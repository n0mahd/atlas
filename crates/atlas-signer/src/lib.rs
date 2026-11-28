//! Passkeys for the Atlas hub.
//!
//! One owner, any number of passkeys (enrol at least two: decision 0007). The credentials
//! live in a JSON file in the hub's state folder, readable only by the hub's user. This
//! crate runs the WebAuthn ceremonies; sessions and HTTP are the web layer's job.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

pub mod setup;

use serde::{Deserialize, Serialize};
pub use webauthn_rs::prelude::{
    CreationChallengeResponse, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, RequestChallengeResponse, Url,
};
use webauthn_rs::prelude::{Passkey, Uuid};
use webauthn_rs::{Webauthn, WebauthnBuilder};

#[derive(Debug, thiserror::Error)]
pub enum SignerError {
    #[error("the hub's public address isn't usable for passkeys: {0}")]
    Origin(String),
    #[error("the passkey check failed")]
    Ceremony(#[from] webauthn_rs::prelude::WebauthnError),
    #[error("can't read or write the passkey file: {0}")]
    Store(String),
    #[error("no passkey with that id")]
    UnknownKey,
    #[error("the last passkey can't be removed, or nobody could sign in")]
    LastKey,
    #[error("This passkey can't sign updates. Add a passkey that can sign (ES256 or Ed25519).")]
    CantSign,
}

/// COSE ES256, the one algorithm enrolment asks authenticators for.
const ES256: i64 = -7;

/// What the UI shows about a stored passkey. Never the key material.
#[derive(Clone, Debug)]
pub struct KeyInfo {
    /// A stable id for the key (its credential id, base64url).
    pub id: String,
    pub name: String,
    /// Seconds since the Unix epoch.
    pub added: u64,
    pub last_used: Option<u64>,
    /// How it was enrolled: the Tailscale login on first run, "setup link", or "signed in".
    pub added_by: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredKey {
    name: String,
    added: u64,
    last_used: Option<u64>,
    #[serde(default)]
    added_by: Option<String>,
    /// Whether the owner has been told at a sign-in that this key was added.
    #[serde(default = "yes")]
    announced: bool,
    passkey: Passkey,
}

const fn yes() -> bool {
    true
}

#[derive(Serialize, Deserialize)]
struct StoreFile {
    /// The owner's WebAuthn user handle, fixed for the life of the store.
    user_id: Uuid,
    keys: Vec<StoredKey>,
}

/// The owner's passkeys and the ceremonies that use them.
pub struct Signer {
    webauthn: Webauthn,
    path: PathBuf,
    file: Mutex<StoreFile>,
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signer")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A key's credential id as unpadded base64url, the form browsers use.
fn key_id(p: &Passkey) -> String {
    b64url(p.cred_id().as_ref())
}

fn info(k: &StoredKey) -> KeyInfo {
    KeyInfo {
        id: key_id(&k.passkey),
        name: k.name.clone(),
        added: k.added,
        last_used: k.last_used,
        added_by: k.added_by.clone(),
    }
}

/// The key's algorithm and raw public key, if updates can be signed with it: ES256 on
/// P-256 or Ed25519, the kinds `atlas-verify` checks.
fn signing_key(p: &Passkey) -> Option<(&'static str, Vec<u8>)> {
    use webauthn_rs::prelude::{COSEKeyType, ECDSACurve, EDDSACurve};
    match &p.get_public_key().key {
        COSEKeyType::EC_EC2(ec)
            if ec.curve == ECDSACurve::SECP256R1 && ec.x.len() == 32 && ec.y.len() == 32 =>
        {
            let mut raw = Vec::with_capacity(65);
            raw.push(0x04);
            raw.extend_from_slice(&ec.x);
            raw.extend_from_slice(&ec.y);
            Some(("es256", raw))
        }
        COSEKeyType::EC_OKP(okp) if okp.curve == EDDSACurve::ED25519 && okp.x.len() == 32 => {
            Some(("eddsa", okp.x.clone()))
        }
        _ => None,
    }
}

/// Unpadded base64url, the form WebAuthn uses on the wire.
#[must_use]
pub fn b64url(bytes: &[u8]) -> String {
    const ABC: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len() * 4 / 3 + 2);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |acc, (i, b)| acc | u32::from(*b) << (16 - 8 * i));
        let len = chunk.len() + 1;
        for i in 0..len {
            out.push(ABC[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

impl Signer {
    /// Opens (or starts) the passkey store in `state_dir` for a hub served at `origin`,
    /// such as `https://hub.example.ts.net:7443`. The relying party is the origin's host.
    ///
    /// # Errors
    /// If the origin has no host, or the store can't be read.
    pub fn open(origin: &Url, state_dir: &Path) -> Result<Self, SignerError> {
        let host = origin
            .host_str()
            .ok_or_else(|| SignerError::Origin(origin.to_string()))?;
        let webauthn = WebauthnBuilder::new(host, origin)
            .and_then(|b| b.rp_name("Atlas").build())
            .map_err(|e| SignerError::Origin(e.to_string()))?;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(state_dir)
            .map_err(|e| SignerError::Store(e.to_string()))?;
        let path = state_dir.join("passkeys.json");
        let file = match fs::read(&path) {
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|e| SignerError::Store(e.to_string()))?
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => StoreFile {
                user_id: Uuid::new_v4(),
                keys: Vec::new(),
            },
            Err(e) => return Err(SignerError::Store(e.to_string())),
        };
        Ok(Self {
            webauthn,
            path,
            file: Mutex::new(file),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, StoreFile> {
        self.file
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Writes the store atomically, readable only by this user.
    fn save(&self, file: &StoreFile) -> Result<(), SignerError> {
        let err = |e: std::io::Error| SignerError::Store(e.to_string());
        let bytes =
            serde_json::to_vec_pretty(file).map_err(|e| SignerError::Store(e.to_string()))?;
        let tmp = self.path.with_extension("json.tmp");
        let mut out = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(err)?;
        out.write_all(&bytes).map_err(err)?;
        out.sync_all().map_err(err)?;
        fs::rename(&tmp, &self.path).map_err(err)
    }

    #[must_use]
    pub fn has_keys(&self) -> bool {
        !self.lock().keys.is_empty()
    }

    #[must_use]
    pub fn keys(&self) -> Vec<KeyInfo> {
        self.lock().keys.iter().map(info).collect()
    }

    /// Starts enrolling a new passkey. Existing keys are excluded so one authenticator
    /// isn't enrolled twice.
    ///
    /// # Errors
    /// If WebAuthn can't build the challenge.
    pub fn start_enrol(
        &self,
    ) -> Result<(CreationChallengeResponse, PasskeyRegistration), SignerError> {
        let file = self.lock();
        let exclude: Vec<_> = file
            .keys
            .iter()
            .map(|k| k.passkey.cred_id().clone())
            .collect();
        let (mut options, state) = self.webauthn.start_passkey_registration(
            file.user_id,
            "owner",
            "Atlas owner",
            (!exclude.is_empty()).then_some(exclude),
        )?;
        // Updates are signed with ES256 or Ed25519 (decision 0019), and browsers offer
        // ES256 everywhere, so that is the one kind asked for.
        options
            .public_key
            .pub_key_cred_params
            .retain(|p| p.alg == ES256);
        Ok((options, state))
    }

    /// Finishes enrolment and stores the passkey under `name`.
    ///
    /// # Errors
    /// If the authenticator's answer doesn't verify or the store can't be written.
    pub fn finish_enrol(
        &self,
        answer: &RegisterPublicKeyCredential,
        state: &PasskeyRegistration,
        name: &str,
        added_by: &str,
    ) -> Result<KeyInfo, SignerError> {
        let passkey = self.webauthn.finish_passkey_registration(answer, state)?;
        if signing_key(&passkey).is_none() {
            return Err(SignerError::CantSign);
        }
        let mut file = self.lock();
        let name: String = name
            .trim()
            .chars()
            .filter(|c| !c.is_control())
            .take(60)
            .collect();
        let name = if name.is_empty() {
            format!("Passkey {}", file.keys.len() + 1)
        } else {
            name
        };
        let info = KeyInfo {
            id: key_id(&passkey),
            name: name.clone(),
            added: now(),
            last_used: None,
            added_by: Some(added_by.to_owned()),
        };
        // The first key is the owner's own; any later one is announced at a sign-in.
        let announced = file.keys.is_empty();
        file.keys.push(StoredKey {
            name,
            added: info.added,
            last_used: None,
            added_by: info.added_by.clone(),
            announced,
            passkey,
        });
        self.save(&file)?;
        Ok(info)
    }

    /// Starts a sign-in. None when no passkey is enrolled yet.
    ///
    /// # Errors
    /// If WebAuthn can't build the challenge.
    pub fn start_sign_in(
        &self,
    ) -> Result<Option<(RequestChallengeResponse, PasskeyAuthentication)>, SignerError> {
        let file = self.lock();
        if file.keys.is_empty() {
            return Ok(None);
        }
        let keys: Vec<Passkey> = file.keys.iter().map(|k| k.passkey.clone()).collect();
        Ok(Some(self.webauthn.start_passkey_authentication(&keys)?))
    }

    /// Finishes a sign-in, updating the key's counter and last use.
    ///
    /// # Errors
    /// If the answer doesn't verify (wrong key, replay, no user verification).
    pub fn finish_sign_in(
        &self,
        answer: &PublicKeyCredential,
        state: &PasskeyAuthentication,
    ) -> Result<KeyInfo, SignerError> {
        let result = self.webauthn.finish_passkey_authentication(answer, state)?;
        let mut file = self.lock();
        let key = file
            .keys
            .iter_mut()
            .find(|k| k.passkey.cred_id() == result.cred_id())
            .ok_or(SignerError::UnknownKey)?;
        key.passkey.update_credential(&result);
        key.last_used = Some(now());
        let used = info(key);
        self.save(&file)?;
        Ok(used)
    }

    /// The owner's passkeys as public keys for release signatures, in the form clients
    /// keep (decision 0007). Enrolment admits only keys that can sign, so this is every
    /// key; one of another kind from an older store is left out.
    #[must_use]
    pub fn owner_keys(&self) -> Vec<atlas_core::release::OwnerKey> {
        self.lock()
            .keys
            .iter()
            .filter_map(|k| {
                let (alg, raw) = signing_key(&k.passkey)?;
                Some(atlas_core::release::OwnerKey {
                    credential_id: key_id(&k.passkey),
                    alg: alg.to_owned(),
                    public_key: b64url(&raw),
                    name: k.name.clone(),
                })
            })
            .collect()
    }

    /// Keys added since the owner last signed in with another key, now marked as told.
    /// A key is not announced to its own sign-in, so the owner hears of it from a key
    /// they already had.
    ///
    /// # Errors
    /// If the store can't be written.
    pub fn take_new_keys(&self, signed_in_with: &str) -> Result<Vec<KeyInfo>, SignerError> {
        let mut file = self.lock();
        let mut new = Vec::new();
        for k in &mut file.keys {
            if !k.announced && key_id(&k.passkey) != signed_in_with {
                k.announced = true;
                new.push(info(k));
            }
        }
        if !new.is_empty() {
            self.save(&file)?;
        }
        Ok(new)
    }

    /// Removes a passkey. The last one can't be removed.
    ///
    /// # Errors
    /// Unknown id, last key, or the store can't be written.
    pub fn remove(&self, id: &str) -> Result<(), SignerError> {
        let mut file = self.lock();
        let before = file.keys.len();
        if before <= 1 {
            return Err(SignerError::LastKey);
        }
        file.keys.retain(|k| key_id(&k.passkey) != id);
        if file.keys.len() == before {
            return Err(SignerError::UnknownKey);
        }
        self.save(&file)
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn a_new_store_is_private_and_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let state = tmp.path().join("state");
        let origin = Url::parse("https://hub.example.ts.net:7443").unwrap();
        let signer = Signer::open(&origin, &state).unwrap();
        assert!(!signer.has_keys());
        assert!(signer.start_sign_in().unwrap().is_none());
        let (challenge, _) = signer.start_enrol().unwrap();
        let json = serde_json::to_string(&challenge).unwrap();
        assert!(json.contains("hub.example.ts.net"), "{json}");
        assert_eq!(
            fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert!(matches!(signer.remove("x"), Err(SignerError::LastKey)));
    }

    #[test]
    fn refuses_an_origin_without_a_host() {
        let tmp = tempfile::tempdir().unwrap();
        let origin = Url::parse("data:text/plain,hi").unwrap();
        assert!(Signer::open(&origin, tmp.path()).is_err());
    }
}
