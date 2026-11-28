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
    /// The `last_seen` in `clients.json` now; the one above can be newer, in memory only.
    #[serde(skip)]
    saved_seen: Option<u64>,
}

/// A client checking in with nothing new rewrites `clients.json` at most this often
/// (seconds), so atlasd's every-minute run isn't a disk write on the hub each time. A
/// hub restart can lose up to this much of `last_seen`.
const SEEN_EVERY: u64 = 60;

/// The same report apart from when it was made.
fn same_report(a: &Report, b: &Report) -> bool {
    a.release == b.release && a.harnesses == b.harnesses && a.problems == b.problems
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
        let mut list: Vec<Client> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for c in &mut list {
            c.saved_seen = c.last_seen;
        }
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

    fn save(&self, list: &mut [Client]) -> Result<(), String> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let bytes = serde_json::to_vec_pretty(&*list).map_err(|e| e.to_string())?;
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
        std::fs::rename(&tmp, &self.path).map_err(|e| e.to_string())?;
        for c in list {
            c.saved_seen = c.last_seen;
        }
        Ok(())
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
            saved_seen: None,
        });
        self.save(&mut list)?;
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

    /// Records a client's report (and that it was seen). When nothing but the time moved
    /// (no report, or the same one again), `clients.json` is rewritten at most once a
    /// minute per client; the newer time stays in memory in between.
    ///
    /// # Errors
    /// If the list can't be saved.
    pub fn record(&self, id: &str, report: Option<Report>) -> Result<(), String> {
        self.record_at(id, report, now())
    }

    fn record_at(&self, id: &str, report: Option<Report>, now: u64) -> Result<(), String> {
        let mut list = self.lock();
        let Some(client) = list.iter_mut().find(|c| c.id == id) else {
            return Err("unknown client".to_owned());
        };
        let news = report.as_ref().is_some_and(|r| {
            client
                .report
                .as_ref()
                .is_none_or(|old| !same_report(old, r))
        });
        client.last_seen = Some(now);
        if report.is_some() {
            client.report = report;
        }
        let recent = client
            .saved_seen
            .is_some_and(|t| now.saturating_sub(t) < SEEN_EVERY);
        if !news && recent {
            return Ok(());
        }
        self.save(&mut list)
    }

    /// Forgets a machine; its token stops working.
    ///
    /// # Errors
    /// If the list can't be saved.
    pub fn remove(&self, id: &str) -> Result<(), String> {
        let mut list = self.lock();
        list.retain(|c| c.id != id);
        self.save(&mut list)
    }

    /// Forgets every paired client on `machine` (case-insensitive); their tokens stop working.
    /// Returns how many clients were removed.
    ///
    /// # Errors
    /// If the list can't be saved.
    pub fn remove_machine(&self, machine: &str) -> Result<usize, String> {
        let mut list = self.lock();
        let before = list.len();
        list.retain(|c| !c.machine.eq_ignore_ascii_case(machine));
        let removed = before - list.len();
        if removed > 0 {
            self.save(&mut list)?;
        }
        Ok(removed)
    }

    #[must_use]
    pub fn list(&self) -> Vec<Client> {
        self.lock().clone()
    }
}

/// Checks a machine name (hostname-style label, no spaces or shell metacharacters).
///
/// # Errors
/// A plain message when the name is empty or contains disallowed characters.
pub fn validate_machine_name(text: &str) -> Result<String, String> {
    let name = text.trim();
    if name.is_empty() {
        return Err("Type a machine name, such as laptop.".to_owned());
    }
    if name.len() > 64 {
        return Err("Machine names can be at most 64 characters.".to_owned());
    }
    let mut chars = name.chars();
    let ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if ok {
        Ok(name.to_owned())
    } else {
        Err(format!(
            "\"{name}\" isn't a valid machine name. Use letters, digits, dots, hyphens or underscores."
        ))
    }
}

#[derive(Debug)]
struct MachinesState {
    persisted: bool,
    names: Vec<String>,
}

/// Tracked machine names (shown on `/machines` even before pairing), persisted in
/// `<state_dir>/machines.json` and seeded once from `hub.toml`'s `machines` list.
#[derive(Debug)]
pub struct Machines {
    path: PathBuf,
    state: Mutex<MachinesState>,
}

impl Machines {
    #[must_use]
    pub fn open(state_dir: &std::path::Path) -> Self {
        let path = state_dir.join("machines.json");
        let loaded: Option<Vec<String>> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let state = match loaded {
            Some(names) => MachinesState {
                persisted: true,
                names,
            },
            None => MachinesState {
                persisted: false,
                names: Vec::new(),
            },
        };
        Self {
            path,
            state: Mutex::new(state),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, MachinesState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn save(&self, names: &[String]) -> Result<(), String> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(names).map_err(|e| e.to_string())?;
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

    /// Seeds the list from `hub.toml` if `machines.json` has not been created yet.
    pub fn seed(&self, initial: &[String]) {
        let mut state = self.lock();
        if state.persisted || !state.names.is_empty() {
            return;
        }
        for raw in initial {
            if let Ok(name) = validate_machine_name(raw)
                && !state.names.iter().any(|n| n.eq_ignore_ascii_case(&name))
            {
                state.names.push(name);
            }
        }
    }

    #[must_use]
    pub fn list(&self) -> Vec<String> {
        self.lock().names.clone()
    }

    /// Adds a machine name to the tracked list.
    ///
    /// # Errors
    /// If the name is invalid or the file can't be saved.
    pub fn add(&self, raw: &str) -> Result<String, String> {
        let name = validate_machine_name(raw)?;
        let mut state = self.lock();
        if let Some(existing) = state
            .names
            .iter()
            .find(|n| n.eq_ignore_ascii_case(&name))
            .cloned()
        {
            return Ok(existing);
        }
        state.names.push(name.clone());
        self.save(&state.names)?;
        state.persisted = true;
        Ok(name)
    }

    /// Removes a machine name (case-insensitive) from the tracked list.
    ///
    /// # Errors
    /// If the file can't be saved.
    pub fn remove(&self, name: &str) -> Result<bool, String> {
        let mut state = self.lock();
        let before = state.names.len();
        state.names.retain(|n| !n.eq_ignore_ascii_case(name));
        let removed = state.names.len() != before;
        if removed {
            self.save(&state.names)?;
            state.persisted = true;
        }
        Ok(removed)
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
        assert_eq!(clients.remove_machine("BOX").unwrap(), 1);
        assert!(clients.list().is_empty());
    }

    #[test]
    fn checking_in_with_nothing_new_writes_at_most_once_a_minute() {
        let tmp = tempfile::tempdir().unwrap();
        let clients = Clients::open(tmp.path());
        let (id, _) = clients.pair("box", "me").unwrap();
        let on_disk = || Clients::open(tmp.path()).list()[0].clone();
        let report = |at, problems: &[&str]| Report {
            release: Some(1),
            at,
            harnesses: Vec::new(),
            problems: problems.iter().map(|p| (*p).to_owned()).collect(),
            outside: Vec::new(),
            kept: Vec::new(),
        };
        let t = 1_000_000;

        // Two release GETs within a minute: one write. The second time is still seen.
        clients.record_at(&id, None, t).unwrap();
        clients.record_at(&id, None, t + 30).unwrap();
        assert_eq!(on_disk().last_seen, Some(t));
        assert_eq!(clients.list()[0].last_seen, Some(t + 30));

        // A first report is news; the same report a little later isn't.
        clients
            .record_at(&id, Some(report(t + 31, &[])), t + 31)
            .unwrap();
        assert_eq!(on_disk().last_seen, Some(t + 31));
        clients
            .record_at(&id, Some(report(t + 40, &[])), t + 40)
            .unwrap();
        assert_eq!(on_disk().last_seen, Some(t + 31));
        assert_eq!(clients.list()[0].report.as_ref().unwrap().at, t + 40);

        // A report that says something new is written at once.
        clients
            .record_at(&id, Some(report(t + 45, &["broken"])), t + 45)
            .unwrap();
        assert_eq!(on_disk().report.unwrap().problems, ["broken"]);

        // A minute after the last write, the time is written again.
        clients.record_at(&id, None, t + 104).unwrap();
        assert_eq!(on_disk().last_seen, Some(t + 45));
        clients.record_at(&id, None, t + 105).unwrap();
        assert_eq!(on_disk().last_seen, Some(t + 105));
    }

    #[test]
    fn machines_seed_add_and_remove_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let m = Machines::open(tmp.path());
        m.seed(&["laptop".to_owned(), "server".to_owned()]);
        assert_eq!(m.list(), vec!["laptop", "server"]);
        assert!(m.add("bad name").is_err());
        assert!(m.add("-bad").is_err());
        assert_eq!(m.add("workstation").unwrap(), "workstation");
        // Duplicate (case-insensitive) returns existing name without duplicating.
        assert_eq!(m.add("LAPTOP").unwrap(), "laptop");
        assert_eq!(m.list(), vec!["laptop", "server", "workstation"]);
        // Removing persists across reloads and stops re-seeding from hub.toml.
        assert!(m.remove("SERVER").unwrap());
        let reloaded = Machines::open(tmp.path());
        reloaded.seed(&["laptop".to_owned(), "server".to_owned()]);
        assert_eq!(reloaded.list(), vec!["laptop", "workstation"]);
    }
}
