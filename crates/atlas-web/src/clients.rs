//! Paired machines: each client (atlasd, one per machine and OS account) has its own
//! token, kept here only as a hash, and its last report.

use std::path::PathBuf;
use std::sync::Mutex;

use atlas_core::distribution::Report;
use axum::http::{HeaderMap, header};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Client {
    pub id: String,
    pub machine: String,
    pub user: String,
    token_hash: String,
    /// Seconds since the Unix epoch.
    pub paired: u64,
    pub last_seen: Option<u64>,
    pub report: Option<Report>,
}

#[derive(Debug)]
pub struct Clients {
    path: PathBuf,
    list: Mutex<Vec<Client>>,
}

#[must_use]
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn clean(text: &str, max: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

impl Clients {
    #[must_use]
    pub fn open(state_dir: &std::path::Path) -> Self {
        let path = state_dir.join("clients.json");
        let list = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        Self {
            path,
            list: Mutex::new(list),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Client>> {
        self.list
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save(&self, list: &[Client]) -> Result<(), String> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
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

    /// Registers a machine and returns its id and token. Pairing again from the same
    /// machine and account replaces the old entry (and its token).
    ///
    /// # Errors
    /// If the list can't be saved.
    pub fn pair(&self, machine: &str, user: &str) -> Result<(String, String), String> {
        let machine = clean(machine, 64);
        let user = clean(user, 64);
        if machine.is_empty() || user.is_empty() {
            return Err("machine and user are required".to_owned());
        }
        let token = crate::auth::token();
        let id = crate::auth::token()[..16].to_owned();
        let mut list = self.lock();
        list.retain(|c| !(c.machine == machine && c.user == user));
        list.push(Client {
            id: id.clone(),
            machine,
            user,
            token_hash: atlas_fs::edit::content_hash(&token),
            paired: now(),
            last_seen: None,
            report: None,
        });
        self.save(&list)?;
        Ok((id, token))
    }

    /// The client behind a request's `Authorization: Bearer` token.
    #[must_use]
    pub fn authenticate(&self, headers: &HeaderMap) -> Option<Client> {
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
        self.lock()
            .iter()
            .find(|c| crate::auth::same_secret(&c.token_hash, &hash))
            .cloned()
    }

    /// Records a client's report (and that it was seen).
    ///
    /// # Errors
    /// If the list can't be saved.
    pub fn record(&self, id: &str, report: Option<Report>) -> Result<(), String> {
        let mut list = self.lock();
        let Some(client) = list.iter_mut().find(|c| c.id == id) else {
            return Err("unknown client".to_owned());
        };
        client.last_seen = Some(now());
        if report.is_some() {
            client.report = report;
        }
        self.save(&list)
    }

    /// Forgets a machine; its token stops working.
    ///
    /// # Errors
    /// If the list can't be saved.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.lock();
        list.retain(|c| c.id != id);
        self.save(&list)
    }

    #[must_use]
    pub fn list(&self) -> Vec<Client> {
        self.lock().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn tokens_identify_clients_and_stay_hashed() {
        let tmp = tempfile::tempdir().unwrap();
        let clients = Clients::open(tmp.path());
        let (id, token) = clients.pair("box", "me").unwrap();
        let on_disk = std::fs::read_to_string(tmp.path().join("clients.json")).unwrap();
        assert!(!on_disk.contains(&token));
        let mut h = HeaderMap::new();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert_eq!(clients.authenticate(&h).unwrap().id, id);
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer nope"),
        );
        assert!(clients.authenticate(&h).is_none());
        // Pairing again replaces the old token.
        let (_, token2) = clients.pair("box", "me").unwrap();
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token}")).unwrap(),
        );
        assert!(clients.authenticate(&h).is_none());
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {token2}")).unwrap(),
        );
        assert!(clients.authenticate(&h).is_some());
        assert_eq!(Clients::open(tmp.path()).list().len(), 1);
    }
}
