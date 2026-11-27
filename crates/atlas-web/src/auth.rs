//! Passkey sign-in for writes (decision 0016).
//!
//! Reading needs only the owner's Tailscale login. Changing anything also needs a session
//! from a passkey sign-in: a `__Host-` cookie (`Secure`, `HttpOnly`, `SameSite=Strict`), a CSRF
//! token per session, and a request whose Origin is the hub's own.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use atlas_signer::{PasskeyAuthentication, PasskeyRegistration};
use axum::http::{HeaderMap, HeaderValue, header};

pub const COOKIE: &str = "__Host-atlas";
const SESSION_TTL: Duration = Duration::from_hours(12);
const FLOW_TTL: Duration = Duration::from_mins(5);
const MAX_SETUP_TRIES: u32 = 5;

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
#[derive(Clone, Debug)]
pub struct Session {
    pub csrf: String,
    expires: Instant,
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
    },
    SignIn {
        state: Box<PasskeyAuthentication>,
        next: String,
    },
}

struct PendingFlow {
    flow: Flow,
    expires: Instant,
}

/// Sessions, ceremonies and the first-passkey setup code. All in memory: a restart signs
/// the owner out, which is fine for one person.
pub struct Auth {
    sessions: Mutex<HashMap<String, Session>>,
    flows: Mutex<HashMap<String, PendingFlow>>,
    setup: Mutex<Setup>,
}

struct Setup {
    code: String,
    failures: u32,
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

fn setup_code() -> String {
    let raw = uuid::Uuid::new_v4().simple().to_string().to_uppercase();
    format!("{}-{}-{}", &raw[..4], &raw[4..8], &raw[8..12])
}

impl Auth {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
            flows: Mutex::new(HashMap::new()),
            setup: Mutex::new(Setup {
                code: setup_code(),
                failures: 0,
            }),
        }
    }

    /// The code that lets the first passkey be enrolled. The hub logs it at start.
    #[must_use]
    pub fn setup_code(&self) -> String {
        self.setup
            .lock()
            .map_or_else(|_| String::new(), |s| s.code.clone())
    }

    /// Checks a setup code. After too many wrong tries the code changes (and is logged).
    pub fn check_setup_code(&self, given: &str) -> bool {
        let Ok(mut setup) = self.setup.lock() else {
            return false;
        };
        let normal = |s: &str| s.trim().to_uppercase().replace([' ', '-'], "");
        if normal(given) == normal(&setup.code) {
            return true;
        }
        setup.failures += 1;
        if setup.failures >= MAX_SETUP_TRIES {
            setup.code = setup_code();
            setup.failures = 0;
            tracing::warn!("too many wrong setup codes; the new code is {}", setup.code);
        }
        false
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

    /// Starts a session and returns its cookie token.
    pub fn sign_in(&self) -> String {
        let id = token();
        if let Ok(mut sessions) = self.sessions.lock() {
            let now = Instant::now();
            sessions.retain(|_, s| s.expires > now);
            sessions.insert(
                id.clone(),
                Session {
                    csrf: token(),
                    expires: now + SESSION_TTL,
                },
            );
        }
        id
    }

    pub fn sign_out(&self, headers: &HeaderMap) {
        if let (Some(id), Ok(mut sessions)) = (cookie(headers), self.sessions.lock()) {
            sessions.remove(&id);
        }
    }

    /// The session behind this request's cookie, if it's live.
    #[must_use]
    pub fn session(&self, headers: &HeaderMap) -> Option<Session> {
        let id = cookie(headers)?;
        let sessions = self.sessions.lock().ok()?;
        sessions
            .get(&id)
            .filter(|s| s.expires > Instant::now())
            .cloned()
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

/// A safe place to go after signing in: a local path only.
#[must_use]
pub fn safe_next(next: &str) -> String {
    let ok = next.starts_with('/') && !next.starts_with("//") && !next.contains(['\\', '\r', '\n']);
    if ok { next.to_owned() } else { "/".to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_round_trip_through_the_cookie() {
        let auth = Auth::new();
        let id = auth.sign_in();
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("other=1; {COOKIE}={id}")).unwrap(),
        );
        assert!(auth.session(&headers).is_some());
        auth.sign_out(&headers);
        assert!(auth.session(&headers).is_none());
    }

    #[test]
    fn setup_code_rotates_after_wrong_guesses() {
        let auth = Auth::new();
        let code = auth.setup_code();
        assert!(auth.check_setup_code(&code.to_lowercase().replace('-', " ")));
        for _ in 0..MAX_SETUP_TRIES {
            assert!(!auth.check_setup_code("nope"));
        }
        assert_ne!(auth.setup_code(), code);
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
        });
        assert!(auth.take_flow(&id).is_some());
        assert!(auth.take_flow(&id).is_none());
    }
}
