//! Passkey-issued admin tokens for the remote CLI (`atlasd auth` / `atlasd machines`).
//!
//! Each token is kept on disk only as a SHA-256 hash (mode 0600), separated from paired
//! client tokens so an ordinary `atlasd` sync login cannot add or remove other machines.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use axum::http::{HeaderMap, header};
use serde::{Deserialize, Serialize};

/// Seconds between writes of a token's last use.
const LAST_USED_EVERY: u64 = 60;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CliToken {
    pub id: String,
    pub name: String,
    token_hash: String,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub last_used: Option<u64>,
}

#[derive(Debug)]
pub struct CliTokens {
    path: PathBuf,
    list: Mutex<Vec<CliToken>>,
}

fn clean_name(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(60)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl CliTokens {
    #[must_use]
    pub fn open(state_dir: &Path) -> Self {
        let path = state_dir.join("cli-tokens.json");
        let list = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            list: Mutex::new(list),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<CliToken>> {
        let mut guard = self
            .list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Reload if the file exists so `atlas-hub token <name>` works while the hub is running.
        if let Ok(bytes) = std::fs::read(&self.path)
            && let Ok(disk) = serde_json::from_slice(&bytes)
        {
            *guard = disk;
        }
        guard
    }

    fn save(&self, list: &[CliToken]) -> Result<(), String> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(list).map_err(|e| e.to_string())?;
        let tmp = self.path.with_extension("json.tmp");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        f.write_all(&bytes)
            .and_then(|()| f.sync_all())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())
    }

    /// Creates a new revocable CLI token and returns the stored metadata plus the raw
    /// bearer token (shown once to the owner).
    ///
    /// # Errors
    /// If the name is empty or the token list can't be saved.
    pub fn create(&self, name: &str) -> Result<(CliToken, String), String> {
        let name = clean_name(name);
        if name.is_empty() {
            return Err("Name the token first, such as laptop.".to_owned());
        }
        let raw = crate::auth::token();
        let id = crate::auth::token()[..16].to_owned();
        let record = CliToken {
            id,
            name,
            token_hash: atlas_fs::edit::content_hash(&raw),
            created: crate::clients::now(),
            last_used: None,
        };
        let mut list = self.lock();
        list.push(record.clone());
        self.save(&list)?;
        Ok((record, raw))
    }

    /// Checks a request's `Authorization: Bearer` header against active CLI tokens.
    #[must_use]
    pub fn authenticate(&self, headers: &HeaderMap) -> Option<CliToken> {
        let token = headers
            .get(header::AUTHORIZATION)?
            .to_str()
            .ok()?
            .strip_prefix("Bearer ")?
            .trim();
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        let hash = atlas_fs::edit::content_hash(token);
        let mut list = self.lock();
        let found = list
            .iter_mut()
            .find(|t| crate::auth::same_secret(&t.token_hash, &hash))?;
        // Recorded on use, at most once a minute per token, so a busy CLI doesn't
        // rewrite the file on every call.
        let now = crate::clients::now();
        if found
            .last_used
            .is_none_or(|t| now.saturating_sub(t) >= LAST_USED_EVERY)
        {
            found.last_used = Some(now);
            let record = found.clone();
            let _ = self.save(&list);
            return Some(record);
        }
        Some(found.clone())
    }

    /// Revokes a CLI token by id.
    ///
    /// # Errors
    /// If the updated list can't be saved.
    pub fn remove(&self, id: &str) -> Result<bool, String> {
        let mut list = self.lock();
        let before = list.len();
        list.retain(|t| t.id != id);
        let removed = list.len() != before;
        if removed {
            self.save(&list)?;
        }
        Ok(removed)
    }

    #[must_use]
    pub fn list(&self) -> Vec<CliToken> {
        self.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn tokens_stay_hashed_authenticate_and_revoke() {
        let tmp = tempfile::tempdir().unwrap();
        let tokens = CliTokens::open(tmp.path());
        assert!(tokens.create("   ").is_err());

        let (rec, raw) = tokens.create("workstation").unwrap();
        let on_disk = std::fs::read_to_string(tmp.path().join("cli-tokens.json")).unwrap();
        assert!(!on_disk.contains(&raw));

        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {raw}")).unwrap(),
        );
        let authed = tokens.authenticate(&h).unwrap();
        assert_eq!(authed.id, rec.id);
        assert!(authed.last_used.is_some());

        // Reload from disk preserves it.
        let reloaded = CliTokens::open(tmp.path());
        assert!(reloaded.authenticate(&h).is_some());

        // Revoking invalidates it.
        assert!(reloaded.remove(&rec.id).unwrap());
        assert!(!reloaded.remove(&rec.id).unwrap());
        assert!(reloaded.authenticate(&h).is_none());
    }
}
