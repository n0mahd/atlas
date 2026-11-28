//! Passkey sign-in for writes (decision 0016).
//!
//! Reading needs only the owner's Tailscale login. Changing anything also needs a session
//! from a passkey sign-in: a `__Host-` cookie (`Secure`, `HttpOnly`, `SameSite=Strict`), a CSRF
//! token per session, and a request whose Origin is the hub's own.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest as _, Sha256};

use atlas_signer::{PasskeyAuthentication, PasskeyRegistration};
use axum::http::{HeaderMap, HeaderValue, header};

pub const COOKIE: &str = "__Host-atlas";
const SESSION_TTL: Duration = Duration::from_hours(12);
const FLOW_TTL: Duration = Duration::from_mins(5);
/// How long a fresh passkey tap stands for the one action it was made for.
const GRANT_TTL: Duration = Duration::from_mins(2);

/// A random token: two v4 UUIDs, 244 random bits.
#[must_use]
pub fn token() -> String {
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}

/// A signed-in session.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Session {
    pub csrf: String,
    /// Unix seconds. Wall-clock, not `Instant`, so a session outlives a hub restart.
    expires: u64,
    /// The passkey (credential id) that opened this session; removing it ends it.
    #[serde(default)]
    pub credential: String,
    /// Shown once on the next page, such as "A passkey was added: …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Sessions are kept under the SHA-256 of their cookie token, so the file on disk can't
/// be replayed as a cookie.
fn stored_key(id: &str) -> String {
    Sha256::digest(id.as_bytes())
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// A passkey ceremony in progress, between its start and finish requests.
#[allow(
    missing_debug_implementations,
    reason = "holds WebAuthn state that must not be logged"
)]
pub enum Flow {
    Enrol {
        state: Box<PasskeyRegistration>,
        name: String,
        /// Who is enrolling: the Tailscale login, "setup link" or "signed in".
        added_by: String,
        /// What allowed the enrolment to start; finishing checks the same thing again.
        authority: Authority,
    },
    SignIn {
        state: Box<PasskeyAuthentication>,
        next: String,
    },
    /// A fresh tap from an enrolled passkey for one sensitive action.
    Confirm {
        state: Box<PasskeyAuthentication>,
        action: String,
        /// The session (its key) that asked; only it can finish.
        session: String,
    },
}

/// What allowed an enrolment to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authority {
    /// No passkey yet, inside the first-run window.
    FirstRun,
    /// A setup link from `atlas-hub setup`, by its token.
    SetupLink(String),
    /// A signed-in session, by its key, with a fresh tap from an enrolled passkey.
    Session(String),
}

struct Grant {
    session: String,
    action: String,
    expires: Instant,
}

struct PendingFlow {
    flow: Flow,
    expires: Instant,
}

/// Sessions and passkey ceremonies. Sessions are saved to `sessions.json` in the hub's
/// state folder (0600, hashed ids) so restarting or redeploying the hub doesn't sign the
/// owner out; ceremonies stay in memory, since they last minutes.
pub struct Auth {
    sessions: Mutex<HashMap<String, Session>>,
    flows: Mutex<HashMap<String, PendingFlow>>,
    grants: Mutex<HashMap<String, Grant>>,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Auth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Auth")
    }
}

impl Default for Auth {
    fn default() -> Self {
        Self::new()
    }
}

impl Auth {
    /// Sessions in memory only.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            flows: Mutex::new(HashMap::new()),
            grants: Mutex::new(HashMap::new()),
            path: None,
        }
    }

    /// Sessions saved in `state_dir/sessions.json`, loading any still live.
    #[must_use]
    pub fn open(state_dir: &Path) -> Self {
        let path = state_dir.join("sessions.json");
        let now = now_secs();
        let mut sessions: HashMap<String, Session> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        // Every session names the passkey that opened it, so removing a key can end them.
        sessions.retain(|_, s| s.expires > now && !s.credential.is_empty());
        Self {
            sessions: Mutex::new(sessions),
            flows: Mutex::new(HashMap::new()),
            grants: Mutex::new(HashMap::new()),
            path: Some(path),
        }
    }

    fn save(&self, sessions: &HashMap<String, Session>) {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let Some(path) = &self.path else { return };
        let result = (|| -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            let bytes = serde_json::to_vec(sessions).map_err(std::io::Error::other)?;
            let tmp = path.with_extension("json.tmp");
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            f.write_all(&bytes)?;
            f.sync_all()?;
            std::fs::rename(&tmp, path)
        })();
        if let Err(e) = result {
            tracing::warn!("couldn't save sessions: {e}");
        }
    }

    pub fn start_flow(&self, flow: Flow) -> String {
        let id = token();
        if let Ok(mut flows) = self.flows.lock() {
            let now = Instant::now();
            flows.retain(|_, f| f.expires > now);
            flows.insert(
                id.clone(),
                PendingFlow {
                    flow,
                    expires: now + FLOW_TTL,
                },
            );
        }
        id
    }

    /// Takes a ceremony by id; each can be finished once.
    pub fn take_flow(&self, id: &str) -> Option<Flow> {
        let mut flows = self.flows.lock().ok()?;
        let pending = flows.remove(id)?;
        (pending.expires > Instant::now()).then_some(pending.flow)
    }

    /// Starts a session opened by passkey `credential` and returns its cookie token.
    /// `notice` is shown once on the first page it opens.
    pub fn sign_in(&self, credential: &str, notice: Option<String>) -> String {
        let id = token();
        if let Ok(mut sessions) = self.sessions.lock() {
            let now = now_secs();
            sessions.retain(|_, s| s.expires > now);
            sessions.insert(
                stored_key(&id),
                Session {
                    csrf: token(),
                    expires: now + SESSION_TTL.as_secs(),
                    credential: credential.to_owned(),
                    notice,
                },
            );
            self.save(&sessions);
        }
        id
    }

    pub fn sign_out(&self, headers: &HeaderMap) {
        if let (Some(id), Ok(mut sessions)) = (cookie(headers), self.sessions.lock())
            && sessions.remove(&stored_key(&id)).is_some()
        {
            self.save(&sessions);
        }
    }

    /// Ends every session opened by passkey `credential`. Returns how many ended.
    pub fn end_sessions_of(&self, credential: &str) -> usize {
        self.end_where(|s| s.credential == credential)
    }

    /// Ends every session, this one included.
    pub fn end_all(&self) -> usize {
        self.end_where(|_| true)
    }

    fn end_where(&self, ends: impl Fn(&Session) -> bool) -> usize {
        let Ok(mut sessions) = self.sessions.lock() else {
            return 0;
        };
        let before = sessions.len();
        sessions.retain(|_, s| !ends(s));
        let ended = before - sessions.len();
        if ended > 0 {
            self.save(&sessions);
        }
        if let Ok(mut grants) = self.grants.lock() {
            grants.retain(|_, g| sessions.contains_key(&g.session));
        }
        ended
    }

    /// The session behind this request's cookie, if it's live.
    #[must_use]
    pub fn session(&self, headers: &HeaderMap) -> Option<Session> {
        let id = cookie(headers)?;
        let sessions = self.sessions.lock().ok()?;
        sessions
            .get(&stored_key(&id))
            .filter(|s| s.expires > now_secs())
            .cloned()
    }

    /// The key of the live session behind this request, which flows and grants are bound to.
    #[must_use]
    pub fn session_key(&self, headers: &HeaderMap) -> Option<String> {
        self.session(headers)?;
        cookie(headers).map(|id| stored_key(&id))
    }

    /// The session's one-time notice, which is then gone.
    pub fn take_notice(&self, headers: &HeaderMap) -> Option<String> {
        let id = cookie(headers)?;
        let mut sessions = self.sessions.lock().ok()?;
        let notice = sessions.get_mut(&stored_key(&id))?.notice.take()?;
        self.save(&sessions);
        Some(notice)
    }

    /// Records a fresh passkey tap for `action` by `session`, and returns its id.
    pub fn grant(&self, session: &str, action: &str) -> String {
        let id = token();
        if let Ok(mut grants) = self.grants.lock() {
            let now = Instant::now();
            grants.retain(|_, g| g.expires > now);
            grants.insert(
                id.clone(),
                Grant {
                    session: session.to_owned(),
                    action: action.to_owned(),
                    expires: now + GRANT_TTL,
                },
            );
        }
        id
    }

    /// Uses up a grant. True only once, within its time, for the same session and action.
    pub fn use_grant(&self, id: &str, session: &str, action: &str) -> bool {
        let Ok(mut grants) = self.grants.lock() else {
            return false;
        };
        grants.remove(id).is_some_and(|g| {
            g.expires > Instant::now() && g.session == session && g.action == action
        })
    }
}

/// The session id from the request's cookie header.
fn cookie(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .find_map(|pair| {
            pair.trim()
                .strip_prefix(COOKIE)?
                .strip_prefix('=')
                .map(str::to_owned)
        })
        .filter(|v| v.len() == 64 && v.bytes().all(|b| b.is_ascii_hexdigit()))
}

#[must_use]
pub fn set_cookie(id: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "{COOKIE}={id}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age={}",
        SESSION_TTL.as_secs()
    ))
    .unwrap_or_else(|_| HeaderValue::from_static(""))
}

#[must_use]
pub fn clear_cookie() -> HeaderValue {
    HeaderValue::from_static("__Host-atlas=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0")
}

/// True when the request comes from a page served by this hub.
#[must_use]
pub fn same_origin(headers: &HeaderMap, origin: &str) -> bool {
    match headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) => o == origin,
        // Browsers send Origin on every POST; without it, accept only an explicit
        // same-origin fetch.
        None => headers.get("sec-fetch-site").and_then(|v| v.to_str().ok()) == Some("same-origin"),
    }
}

/// Compares secrets without stopping at the first difference.
#[must_use]
pub fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

/// A safe place to go after signing in: a path on this hub, else `/`.
///
/// Only plain URL characters are allowed, and the path must still be local once any
/// percent-encoding is decoded, so a browser can't read it as another site's address.
#[must_use]
/// The sign-in page's address that comes back to `next` (kept on this site) afterwards,
/// with `next` encoded so its own query string survives the trip.
pub fn signin_href(next: &str) -> String {
    let next = safe_next(next);
    let mut out = String::from("/signin?next=");
    for b in next.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(char::from(b));
        } else {
            use std::fmt::Write as _;
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

pub fn safe_next(next: &str) -> String {
    if is_local_path(next) {
        next.to_owned()
    } else {
        "/".to_owned()
    }
}

fn is_local_path(next: &str) -> bool {
    const ALLOWED: &[u8] = b"-._~/?=&%+#,:@!$*()";
    let bytes = next.as_bytes();
    if bytes.first() != Some(&b'/')
        || !bytes
            .iter()
            .all(|b| b.is_ascii_alphanumeric() || ALLOWED.contains(b))
    {
        return false;
    }
    // Decode percent-escapes once; what they stand for must be just as plain.
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes
                .get(i + 1..i + 3)
                .and_then(|h| std::str::from_utf8(h).ok())
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            let Some(b) = hex else { return false };
            if b.is_ascii_control() || b == b'\\' || !b.is_ascii() {
                return false;
            }
            decoded.push(b);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    !decoded.starts_with(b"//")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip_through_the_cookie() {
        let auth = Auth::new();
        let id = auth.sign_in("key", None);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=1; {COOKIE}={id}")).unwrap(),
        );
        assert!(auth.session(&headers).is_some());
        auth.sign_out(&headers);
        assert!(auth.session(&headers).is_none());
    }

    /// Redeploying the hub restarts it; that must not sign the owner out.
    #[test]
    fn sessions_survive_a_restart_and_are_stored_hashed() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let id = Auth::open(dir.path()).sign_in("key", None);
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("{COOKIE}={id}")).unwrap(),
        );

        let file = dir.path().join("sessions.json");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            !text.contains(&id),
            "the raw session id must not be on disk"
        );
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let restarted = Auth::open(dir.path());
        assert!(restarted.session(&headers).is_some());
        restarted.sign_out(&headers);
        assert!(Auth::open(dir.path()).session(&headers).is_none());
    }

    #[test]
    fn signin_links_keep_the_whole_next_address() {
        assert_eq!(signin_href("/settings"), "/signin?next=/settings");
        assert_eq!(
            signin_href("/skills/updates?scope=x&open=a"),
            "/signin?next=/skills/updates%3Fscope%3Dx%26open%3Da"
        );
        assert_eq!(signin_href("//evil.test"), "/signin?next=/");
    }

    #[test]
    fn next_stays_on_this_site() {
        assert_eq!(safe_next("/skills?scope=x"), "/skills?scope=x");
        for bad in [
            "//evil.example",
            "https://evil.example",
            "/\\evil",
            "javascript:x",
        ] {
            assert_eq!(safe_next(bad), "/");
        }
    }

    #[test]
    fn origin_must_match() {
        let mut h = HeaderMap::new();
        assert!(!same_origin(&h, "https://hub"));
        h.insert(header::ORIGIN, HeaderValue::from_static("https://evil"));
        assert!(!same_origin(&h, "https://hub"));
        h.insert(header::ORIGIN, HeaderValue::from_static("https://hub"));
        assert!(same_origin(&h, "https://hub"));
    }

    #[test]
    fn flows_finish_once() {
        let tmp = std::env::temp_dir().join(format!("atlas-auth-test-{}", token()));
        let origin = atlas_signer::Url::parse("https://hub.example").unwrap();
        let signer = atlas_signer::Signer::open(&origin, &tmp).unwrap();
        let (_, state) = signer.start_enrol().unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
        let auth = Auth::new();
        let id = auth.start_flow(Flow::Enrol {
            state: Box::new(state),
            name: String::new(),
            added_by: String::new(),
            authority: Authority::FirstRun,
        });
        assert!(auth.take_flow(&id).is_some());
        assert!(auth.take_flow(&id).is_none());
    }
}
