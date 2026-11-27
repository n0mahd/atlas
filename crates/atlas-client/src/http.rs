//! The hub's client API (`/api/pair`, `/api/release`, `/api/report`), kept thin.

use std::time::Duration;

use atlas_core::distribution::{PairRequest, PairResponse, Report};
use atlas_core::release::SignedRelease;
use reqwest::StatusCode;
use reqwest::blocking::{Client, Response};

fn client() -> Result<Client, String> {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent(concat!("atlasd/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| e.to_string())
}

fn refused(resp: Response) -> String {
    let status = resp.status();
    let text = resp.text().unwrap_or_default();
    let text = text.trim();
    if text.is_empty() {
        format!("the hub answered {status}")
    } else {
        format!("the hub answered {status}: {text}")
    }
}

/// Pairs with the hub using the owner's one-time code.
///
/// # Errors
/// The hub is unreachable or refused the code.
pub fn pair(hub: &str, request: &PairRequest) -> Result<PairResponse, String> {
    let resp = client()?
        .post(format!("{hub}/api/pair"))
        .json(request)
        .send()
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(refused(resp));
    }
    resp.json()
        .map_err(|e| format!("the hub's answer isn't valid: {e}"))
}

/// The hub's latest signed release; None when it has none yet.
///
/// # Errors
/// The hub is unreachable or refused the token.
pub fn release(hub: &str, token: &str) -> Result<Option<SignedRelease>, String> {
    let resp = client()?
        .get(format!("{hub}/api/release"))
        .bearer_auth(token)
        .send()
        .map_err(|e| e.to_string())?;
    match resp.status() {
        StatusCode::NO_CONTENT => Ok(None),
        s if s.is_success() => resp
            .json()
            .map(Some)
            .map_err(|e| format!("the hub's release isn't valid: {e}")),
        _ => Err(refused(resp)),
    }
}

/// Sends a pass's report.
///
/// # Errors
/// The hub is unreachable or refused it.
pub fn report(hub: &str, token: &str, report: &Report) -> Result<(), String> {
    let resp = client()?
        .post(format!("{hub}/api/report"))
        .bearer_auth(token)
        .json(report)
        .send()
        .map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(refused(resp))
    }
}
