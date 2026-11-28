//! Phase 3 on the hub: updates for the machines (signed releases underneath) sent with
//! the owner's passkey, pairing machines, and the small API their clients use
//! (decisions 0019, 0022).

use std::sync::Arc;

use atlas_core::distribution::{PairRequest, PairResponse, Report};
use atlas_core::release::{Manifest, OwnerKey, Signature, SignedRelease, SourceId};
use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::edit::{CsrfForm, Writer};
use crate::release;
use crate::remote::{self, PAIRING_TTL};
use crate::{AppError, AppState, SshResult, auth};

/// How long a built update waits for its signature.
const PENDING_TTL: std::time::Duration = std::time::Duration::from_mins(10);

/// An update built for the owner, waiting for the passkey tap.
#[derive(Debug, Clone)]
pub struct Pending {
    pub number: u64,
    pub bytes: Vec<u8>,
    pub digest: [u8; 32],
    expires: std::time::Instant,
}

fn writer(s: &AppState) -> Result<&Arc<Writer>, AppError> {
    s.writer.as_ref().ok_or(AppError::NotFound)
}

fn forbidden(msg: &'static str) -> Response {
    (StatusCode::FORBIDDEN, msg).into_response()
}

fn verify_key(k: &OwnerKey) -> Option<atlas_verify::OwnerKey> {
    let alg = match k.alg.as_str() {
        "es256" => atlas_verify::KeyAlg::Es256,
        "eddsa" => atlas_verify::KeyAlg::EdDsa,
        _ => return None,
    };
    Some(atlas_verify::OwnerKey {
        credential_id: k.credential_id.clone(),
        alg,
        public_key: k.public_key.clone(),
    })
}

// ---------------------------------------------------------------- sending updates

/// One sent update, for the history in Settings.
pub(crate) struct UpdateRow {
    pub number: u64,
    pub date: String,
    pub passkey: String,
}

fn decode_manifest(release: &SignedRelease) -> Option<Manifest> {
    serde_json::from_slice(&decode_b64(&release.manifest)?).ok()
}

fn decode_b64(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0u32;
    for b in text.bytes().filter(|b| *b != b'=') {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(u8::try_from((acc >> bits) & 0xff).ok()?);
        }
    }
    Some(out)
}

/// Sent updates, newest first.
pub(crate) fn history(w: &Writer) -> Vec<UpdateRow> {
    let names: std::collections::HashMap<String, String> = w
        .signer
        .owner_keys()
        .into_iter()
        .map(|k| (k.credential_id, k.name))
        .collect();
    w.releases
        .numbers()
        .iter()
        .rev()
        .filter_map(|n| w.releases.get(*n))
        .map(|r| UpdateRow {
            number: r.number,
            date: decode_manifest(&r)
                .map_or_else(String::new, |m| crate::edit::day_month(m.created)),
            passkey: names
                .get(&r.signature.credential_id)
                .cloned()
                .unwrap_or_else(|| "a removed passkey".to_owned()),
        })
        .collect()
}

/// Saved changes that aren't on the machines yet.
pub(crate) enum Waiting {
    /// Nothing has ever been sent.
    NeverSent,
    /// Commits in Knowledge and public skills since the latest update's pins.
    Changes(u32),
}

impl Waiting {
    /// The line the UI shows, or None when the machines have everything.
    pub(crate) fn text(&self) -> Option<String> {
        match self {
            Self::NeverSent => Some("Nothing has been sent to your machines yet".to_owned()),
            Self::Changes(0) => None,
            Self::Changes(1) => Some("1 change isn't on your machines yet".to_owned()),
            Self::Changes(n) => Some(format!("{n} changes aren't on your machines yet")),
        }
    }
}

/// Counts what's waiting. None on a read-only hub.
pub(crate) async fn waiting(s: &AppState) -> Option<Waiting> {
    let w = s.writer.as_ref()?;
    let latest = w.releases.latest().and_then(|r| decode_manifest(&r));
    let Some(latest) = latest else {
        return Some(Waiting::NeverSent);
    };
    let sources = Arc::clone(&s.sources);
    tokio::task::spawn_blocking(move || {
        let mut repos = vec![(SourceId::Knowledge, sources.knowledge.clone())];
        if let Some(p) = &sources.public_skills {
            repos.push((SourceId::PublicSkills, p.clone()));
        }
        let count = repos
            .iter()
            .filter_map(|(id, root)| {
                let pin = latest.source(id)?;
                let repo = atlas_git::Repo::open(root).ok()?;
                let since = repo.commits_since(pin.commit.as_str()).ok().flatten()?;
                // Count only what Send can deliver: Send pushes Knowledge, but public
                // skills reach machines only once the owner has pushed them.
                let held_back = match id {
                    SourceId::PublicSkills => repo.unpushed().ok().flatten().unwrap_or(0),
                    _ => 0,
                };
                Some(since.saturating_sub(held_back))
            })
            .sum();
        Waiting::Changes(count)
    })
    .await
    .ok()
}

#[derive(Serialize)]
struct SignOptions {
    challenge: String,
    #[serde(rename = "rpId")]
    rp_id: String,
    #[serde(rename = "allowCredentials")]
    allow: Vec<AllowedKey>,
    #[serde(rename = "userVerification")]
    user_verification: &'static str,
    timeout: u32,
    /// What this update carries, such as "Update 4 · Knowledge".
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Serialize)]
struct AllowedKey {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct NothingToSend {
    nothing: bool,
    next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

#[derive(Deserialize)]
pub struct SendStart {
    /// Where to go once sent.
    #[serde(default)]
    next: String,
}

fn next_or_settings(next: &str) -> String {
    if next.is_empty() {
        "/settings#updates".to_owned()
    } else {
        auth::safe_next(next)
    }
}

fn csrf_header(headers: &HeaderMap) -> &str {
    headers
        .get("x-atlas-csrf")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
}

fn rp_id(w: &Writer) -> String {
    atlas_signer::Url::parse(w.origin())
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Pushes Knowledge if the remote lacks some of its commits, then builds the update.
/// Returns it with notes for the owner, such as public skills waiting for a push.
fn prepare(
    writer: &Writer,
    sources: &atlas_fs::Sources,
    number: u64,
    origin: &str,
    keys: Vec<OwnerKey>,
) -> Result<(Manifest, Vec<String>), String> {
    let _guard = writer
        .git
        .lock()
        .map_err(|_| "Another save is stuck.".to_owned())?;
    if let Ok(repo) = atlas_git::Repo::open(&sources.knowledge)
        && repo.unpushed().ok().flatten().unwrap_or(0) > 0
    {
        repo.push().map_err(|e| {
            format!("Knowledge couldn't be pushed, so your machines can't fetch it: {e}")
        })?;
    }
    let mut notes = Vec::new();
    let public_unpushed = sources.public_skills.as_ref().is_some_and(|p| {
        atlas_git::Repo::open(p)
            .ok()
            .and_then(|r| r.unpushed().ok().flatten())
            .unwrap_or(0)
            > 0
    });
    if public_unpushed {
        notes.push("Public skills reach your machines after you push them.".to_owned());
    }
    let (manifest, skipped) = release::build(sources, number, origin, keys, crate::clients::now());
    notes.extend(
        skipped
            .iter()
            .map(|k| format!("{} {}.", k.source, k.reason)),
    );
    Ok((manifest, notes))
}

/// Pushes Knowledge if it has commits the remote lacks (never public skills), builds the
/// next update, and asks for the passkey tap whose challenge is its digest. Answers
/// `nothing` when machines already have all of it.
pub(crate) async fn send_start(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SendStart>,
) -> Result<Response, AppError> {
    let w = Arc::clone(writer(&s)?);
    if !w.authorised(&headers, csrf_header(&headers)) {
        return Ok(forbidden("Sign in again to send changes.\n"));
    }
    let next = next_or_settings(&req.next);
    let keys = w.signer.owner_keys();
    if keys.is_empty() {
        return Err(AppError::Bad(
            "Add a passkey that can sign (ES256 or Ed25519) first.".into(),
        ));
    }
    let previous = w.releases.latest().and_then(|r| decode_manifest(&r));
    let number = w.releases.numbers().last().map_or(1, |n| n + 1);
    let sources = Arc::clone(&s.sources);
    let origin = w.origin().to_owned();
    let writer = Arc::clone(&w);
    let built =
        tokio::task::spawn_blocking(move || prepare(&writer, &sources, number, &origin, keys))
            .await
            .map_err(|_| AppError::Internal)?;
    s.invalidate().await;
    let (manifest, notes) = built.map_err(AppError::Bad)?;
    let note = (!notes.is_empty()).then(|| notes.join(" "));
    if manifest.sources.is_empty() {
        return Err(AppError::Bad(format!(
            "Nothing can reach your machines yet. {}",
            note.unwrap_or_default()
        )));
    }
    if previous
        .as_ref()
        .is_some_and(|p| p.sources == manifest.sources && p.owner_keys == manifest.owner_keys)
    {
        return Ok(Json(NothingToSend {
            nothing: true,
            next,
            note,
        })
        .into_response());
    }
    let changed: Vec<String> = manifest
        .sources
        .iter()
        .filter(|src| previous.as_ref().and_then(|p| p.source(&src.id)) != Some(*src))
        .map(|src| release::label(&src.id))
        .collect();
    let summary = if changed.is_empty() {
        format!("Update {number} · passkeys")
    } else {
        format!("Update {number} · {}", changed.join(", "))
    };
    let bytes = serde_json::to_vec(&manifest).map_err(|_| AppError::Internal)?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if let Ok(mut pending) = w.pending.lock() {
        *pending = Some(Pending {
            number,
            bytes,
            digest,
            expires: std::time::Instant::now() + PENDING_TTL,
        });
    }
    let allow = w
        .signer
        .owner_keys()
        .into_iter()
        .map(|k| AllowedKey {
            id: k.credential_id,
            kind: "public-key",
        })
        .collect();
    Ok(Json(SignOptions {
        challenge: atlas_signer::b64url(&digest),
        rp_id: rp_id(&w),
        allow,
        user_verification: "required",
        timeout: 120_000,
        summary,
        note,
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct SendFinish {
    credential: Signature,
    #[serde(default)]
    next: String,
}

#[derive(Serialize)]
struct Done {
    next: String,
}

/// Checks the passkey's signature over the pending update exactly as a client will, then
/// stores it for the machines to fetch.
pub(crate) async fn send_finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SendFinish>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, csrf_header(&headers)) {
        return Ok(forbidden("Sign in again to send changes.\n"));
    }
    let pending = w
        .pending
        .lock()
        .ok()
        .and_then(|mut p| p.take())
        .filter(|p| p.expires > std::time::Instant::now());
    let Some(p) = pending else {
        return Ok(forbidden("That request expired. Send again.\n"));
    };
    let keys: Vec<atlas_verify::OwnerKey> = w
        .signer
        .owner_keys()
        .iter()
        .filter_map(verify_key)
        .collect();
    let rp_id = rp_id(w);
    let assertion = atlas_verify::Assertion {
        credential_id: req.credential.credential_id.clone(),
        authenticator_data: req.credential.authenticator_data.clone(),
        client_data_json: req.credential.client_data_json.clone(),
        signature: req.credential.signature.clone(),
    };
    let expected = atlas_verify::Expected {
        rp_id: &rp_id,
        origin: w.origin(),
        challenge: &p.digest,
    };
    if let Err(e) = atlas_verify::verify(&assertion, &keys, &expected) {
        tracing::warn!("update {} signature refused: {e:?}", p.number);
        return Ok(forbidden(
            "That signature didn't check out, so nothing was sent.\n",
        ));
    }
    let release = SignedRelease {
        number: p.number,
        manifest: atlas_signer::b64url(&p.bytes),
        signature: req.credential,
    };
    w.releases.save(&release).map_err(AppError::Bad)?;
    tracing::warn!("update {} signed and sent", p.number);
    Ok(Json(Done {
        next: next_or_settings(&req.next),
    })
    .into_response())
}

/// The old Releases page lives in Settings now.
pub(crate) async fn releases_moved() -> Redirect {
    Redirect::to("/settings#updates")
}

// ---------------------------------------------------------------- pairing (owner side)

/// Issues a one-time pairing code and shows the command to run on the machine.
pub(crate) async fn pair_code(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, f.csrf()) {
        return Ok(forbidden("Sign in again to pair a machine.\n"));
    }
    let code = issue_pairing_code(w)?;
    crate::machines_page(
        &s,
        &headers,
        Some(remote::install_command(w.origin(), &code)),
        None,
    )
    .await
}

fn issue_pairing_code(w: &Writer) -> Result<String, AppError> {
    atlas_signer::setup::issue_in(
        &w.state_dir,
        atlas_signer::setup::PAIRING_CODES,
        PAIRING_TTL,
    )
    .map_err(|e| AppError::Bad(e.to_string()))
}

#[derive(Deserialize)]
pub(crate) struct SshForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    target: String,
}

/// Installs and pairs atlasd on another machine over SSH, with a fresh pairing code.
pub(crate) async fn pair_ssh(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<SshForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to pair a machine.\n"));
    }
    let target = match remote::Target::parse(&f.target) {
        Ok(t) => t,
        Err(message) => {
            let result = SshResult {
                ok: false,
                message,
                output: String::new(),
                target: f.target.trim().to_owned(),
            };
            return crate::machines_page(&s, &headers, None, Some(result)).await;
        }
    };
    let code = issue_pairing_code(w)?;
    tracing::info!("installing atlasd on {target} over ssh");
    let (result, command) = match remote::install_over_ssh(w.origin(), &code, &target).await {
        Ok(_) => (
            SshResult {
                ok: true,
                message: format!(
                    "Installed and paired on {target}. It shows here once atlasd reports, within a minute."
                ),
                output: String::new(),
                target: String::new(),
            },
            None,
        ),
        Err(e) => {
            tracing::warn!("install on {target} over ssh failed: {}", e.message);
            (
                SshResult {
                    ok: false,
                    message: e.message,
                    output: e.output,
                    target: target.to_string(),
                },
                // The code may still be unused, so offer the command to run by hand.
                Some(remote::install_command(w.origin(), &code)),
            )
        }
    };
    crate::machines_page(&s, &headers, command, Some(result)).await
}

pub(crate) async fn unpair(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, f.csrf()) {
        return Ok(forbidden("Sign in again to remove a machine.\n"));
    }
    w.clients.remove(&id).map_err(AppError::Bad)?;
    Ok(Redirect::to("/machines").into_response())
}

// ---------------------------------------------------------------- client API

pub(crate) async fn api_pair(
    State(s): State<AppState>,
    Json(req): Json<PairRequest>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    // Refuse a wrong address before the code is spent, so the owner can simply retry.
    if !req.hub.is_empty() && req.hub.trim_end_matches('/') != w.origin() {
        return Err(AppError::Bad(format!(
            "This hub's address is {}; pair using that.",
            w.origin()
        )));
    }
    let ok = atlas_signer::setup::redeem_in(
        &w.state_dir,
        atlas_signer::setup::PAIRING_CODES,
        req.code.trim(),
    )
    .map_err(|e| AppError::Bad(e.to_string()))?;
    if !ok {
        return Ok(forbidden(
            "That pairing code has expired or was already used. Make a new one on the hub's Machines page.\n",
        ));
    }
    let (client_id, token) = w
        .clients
        .pair(&req.machine, &req.user)
        .map_err(AppError::Bad)?;
    tracing::warn!("paired {} ({})", req.machine, req.user);
    Ok(Json(PairResponse {
        client_id,
        token,
        hub: w.origin().to_owned(),
        owner_keys: w.signer.owner_keys(),
    })
    .into_response())
}

pub(crate) async fn api_release(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    let Some(client) = w.clients.authenticate(&headers) else {
        return Ok((StatusCode::UNAUTHORIZED, "Unknown client token.\n").into_response());
    };
    let _ = w.clients.record(&client.id, None);
    Ok(match w.releases.latest() {
        Some(r) => Json(r).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    })
}

pub(crate) async fn api_report(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(report): Json<Report>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    let Some(client) = w.clients.authenticate(&headers) else {
        return Ok((StatusCode::UNAUTHORIZED, "Unknown client token.\n").into_response());
    };
    w.clients
        .record(&client.id, Some(report))
        .map_err(AppError::Bad)?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    #[test]
    fn base64url_round_trips() {
        for bytes in [&b""[..], b"a", b"ab", b"abc", b"\xff\x00\x10hello"] {
            assert_eq!(
                super::decode_b64(&atlas_signer::b64url(bytes)).unwrap(),
                bytes
            );
        }
        assert!(super::decode_b64("not base64!").is_none());
    }

    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use tower::ServiceExt;

    async fn call(app: &axum::Router, req: Request<Body>) -> (StatusCode, String) {
        let res = app.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let body = to_bytes(res.into_body(), 1 << 20).await.unwrap();
        (status, String::from_utf8_lossy(&body).into_owned())
    }

    #[tokio::test]
    async fn a_machine_pairs_once_and_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().to_path_buf();
        let k = home.join("Knowledge");
        std::fs::create_dir_all(&k).unwrap();
        let sources = atlas_fs::Sources {
            home: home.clone(),
            knowledge: k,
            public_skills: None,
            projects: Vec::new(),
        };
        let state = home.join("state");
        let writer =
            crate::Writer::open("https://hub.test", &state, std::time::Duration::ZERO).unwrap();
        let app = crate::router(
            crate::AppState::new(sources, "hub", vec!["laptop".into()], Some(writer)),
            crate::Access::new(vec!["owner@github".into()]),
        );
        let json = |path: &str, body: String, token: Option<&str>| {
            let mut r = Request::post(path)
                .header("tailscale-user-login", "owner@github")
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(t) = token {
                r = r.header(header::AUTHORIZATION, format!("Bearer {t}"));
            }
            r.body(Body::from(body)).unwrap()
        };
        let code = atlas_signer::setup::issue_in(
            &state,
            atlas_signer::setup::PAIRING_CODES,
            std::time::Duration::from_mins(15),
        )
        .unwrap();
        let wrong = format!(
            r#"{{"code":"{code}","hub":"https://elsewhere.test","machine":"laptop","user":"me"}}"#
        );
        let (status, body) = call(&app, json("/api/pair", wrong, None)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        assert!(body.contains("https://hub.test"), "names the right address");
        let pair = format!(
            r#"{{"code":"{code}","hub":"https://hub.test/","machine":"laptop","user":"me"}}"#
        );
        let (status, body) = call(&app, json("/api/pair", pair.clone(), None)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let resp: atlas_core::distribution::PairResponse = serde_json::from_str(&body).unwrap();
        assert_eq!(resp.hub, "https://hub.test");
        let (status, _) = call(&app, json("/api/pair", pair, None)).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "a code works once");

        let get = |token: &str| {
            Request::get("/api/release")
                .header("tailscale-user-login", "owner@github")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            call(&app, get(&resp.token)).await.0,
            StatusCode::NO_CONTENT,
            "no release yet"
        );
        assert_eq!(
            call(&app, get(&"0".repeat(64))).await.0,
            StatusCode::UNAUTHORIZED
        );

        let report = r#"{"release":null,"at":0,"harnesses":[{"kind":"codex","present":true,"skills_from_atlas":3,"skills_outside":1,"broken":0,"instructions":"linked","hooks_managed":0}],"problems":["one thing failed"]}"#;
        assert_eq!(
            call(&app, json("/api/report", report.into(), Some(&resp.token)))
                .await
                .0,
            StatusCode::NO_CONTENT
        );
        let page = Request::get("/machines")
            .header("tailscale-user-login", "owner@github")
            .body(Body::empty())
            .unwrap();
        let (_, html) = call(&app, page).await;
        assert!(
            html.contains("3 skills from Atlas") && html.contains("one thing failed"),
            "{html}"
        );
    }

    // ------------------------------------------------------------ sending updates

    use std::path::{Path, PathBuf};

    use atlas_core::release::{Manifest, SourceId};
    use sha2::{Digest, Sha256};

    const ORIGIN: &str = "https://hub.test";
    const LOGIN: &str = "owner@github";

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    /// A repo with one commit, pushed to a local bare remote it tracks.
    fn pushed_repo(home: &Path, name: &str) -> PathBuf {
        let dir = home.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init", "-q", "-b", "main"]);
        git(&dir, &["config", "user.name", "Test"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        std::fs::write(dir.join("AGENTS.md"), "# Rules\n").unwrap();
        git(&dir, &["add", "-A"]);
        git(&dir, &["commit", "-q", "-m", "start"]);
        let bare = home.join(format!("{name}.git"));
        git(home, &["init", "-q", "--bare", bare.to_str().unwrap()]);
        git(&dir, &["remote", "add", "origin", bare.to_str().unwrap()]);
        git(&dir, &["push", "-q", "-u", "origin", "main"]);
        dir
    }

    fn commit(dir: &Path, file: &str) {
        std::fs::write(dir.join(file), file).unwrap();
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", file]);
    }

    /// The owner's passkey, as an authenticator holds it.
    fn owner_key() -> p256::ecdsa::SigningKey {
        p256::ecdsa::SigningKey::from_slice(&[7u8; 32]).unwrap()
    }

    const CRED_ID: &[u8] = &[1, 2, 3, 4];

    /// Writes the hub's passkey store with the owner's one key, named Laptop.
    fn enrol(state: &Path) {
        let point = owner_key().verifying_key().to_sec1_point(false);
        let raw = point.as_bytes();
        let b64 = atlas_signer::b64url;
        let passkey = format!(
            r#"{{"cred":{{"cred_id":"{}","cred":{{"type_":"ES256","key":{{"EC_EC2":{{"curve":"SECP256R1","x":"{}","y":"{}"}}}}}},"counter":0,"transports":null,"user_verified":true,"backup_eligible":false,"backup_state":false,"registration_policy":"required","extensions":{{"cred_protect":"NotRequested","hmac_create_secret":"NotRequested","appid":"NotRequested","cred_props":"NotRequested"}},"attestation":{{"data":"None","metadata":"None"}},"attestation_format":"none"}}}}"#,
            b64(CRED_ID),
            b64(&raw[1..33]),
            b64(&raw[33..65])
        );
        let store = format!(
            r#"{{"user_id":"00000000-0000-4000-8000-000000000001","keys":[{{"name":"Laptop","added":0,"last_used":null,"passkey":{passkey}}}]}}"#
        );
        std::fs::create_dir_all(state).unwrap();
        std::fs::write(state.join("passkeys.json"), store).unwrap();
    }

    /// Signs `challenge` exactly as a browser's passkey prompt would.
    fn assertion(challenge: &[u8]) -> String {
        use p256::ecdsa::signature::Signer as _;
        let b64 = atlas_signer::b64url;
        let mut auth_data = Sha256::digest(b"hub.test").to_vec();
        auth_data.push(0x05);
        auth_data.extend_from_slice(&1u32.to_be_bytes());
        let client = format!(
            r#"{{"type":"webauthn.get","challenge":"{}","origin":"{ORIGIN}"}}"#,
            b64(challenge)
        );
        let mut message = auth_data.clone();
        message.extend_from_slice(&Sha256::digest(client.as_bytes()));
        let sig: p256::ecdsa::Signature = owner_key().sign(&message);
        format!(
            r#"{{"next":"/memory/fact?saved=abc&sent=1","credential":{{"credential_id":"{}","authenticator_data":"{}","client_data_json":"{}","signature":"{}"}}}}"#,
            b64(CRED_ID),
            b64(&auth_data),
            b64(client.as_bytes()),
            b64(sig.to_der().as_bytes())
        )
    }

    struct SendHub {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        knowledge: PathBuf,
        public: PathBuf,
        state: crate::AppState,
        app: axum::Router,
        cookie: String,
        csrf: String,
    }

    impl SendHub {
        fn new() -> Self {
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().to_path_buf();
            let knowledge = pushed_repo(&home, "Knowledge");
            let public = pushed_repo(&home, "skills");
            let shop = pushed_repo(&home, "shop");
            let state_dir = home.join("state");
            enrol(&state_dir);
            let writer =
                crate::Writer::open(ORIGIN, &state_dir, std::time::Duration::ZERO).unwrap();
            let cookie = writer.auth.sign_in();
            let mut h = HeaderMap::new();
            h.insert(
                header::COOKIE,
                format!("{}={cookie}", crate::auth::COOKIE).parse().unwrap(),
            );
            let csrf = writer.auth.session(&h).unwrap().csrf;
            let sources = atlas_fs::Sources {
                home: home.clone(),
                knowledge: knowledge.clone(),
                public_skills: Some(public.clone()),
                projects: vec![atlas_fs::Project {
                    name: atlas_core::kernel::Slug::new("shop").unwrap(),
                    root: shop,
                }],
            };
            let state = crate::AppState::new(sources, "hub", Vec::new(), Some(writer));
            let app = crate::router(state.clone(), crate::Access::new(vec![LOGIN.into()]));
            Self {
                _tmp: tmp,
                home,
                knowledge,
                public,
                state,
                app,
                cookie,
                csrf,
            }
        }

        fn post(&self, path: &str, body: &str, signed_in: bool) -> Request<Body> {
            let mut r = Request::post(path)
                .header("tailscale-user-login", LOGIN)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-atlas-csrf", &self.csrf);
            if signed_in {
                r = r.header(
                    header::COOKIE,
                    format!("{}={}", crate::auth::COOKIE, self.cookie),
                );
            }
            r.body(Body::from(body.to_owned())).unwrap()
        }

        fn get(&self, path: &str) -> Request<Body> {
            Request::get(path)
                .header("tailscale-user-login", LOGIN)
                .header(
                    header::COOKIE,
                    format!("{}={}", crate::auth::COOKIE, self.cookie),
                )
                .body(Body::empty())
                .unwrap()
        }

        fn writer(&self) -> &crate::Writer {
            self.state.writer.as_deref().unwrap()
        }

        fn pending_bytes(&self) -> Vec<u8> {
            self.writer()
                .pending
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .bytes
                .clone()
        }
    }

    use axum::http::HeaderMap;

    #[tokio::test]
    async fn sending_needs_a_session() {
        let hub = SendHub::new();
        let (status, _) = call(&hub.app, hub.post("/send/start", "{}", false)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(&hub.app, hub.post("/send/finish", "{}", false)).await;
        assert!(status.is_client_error(), "{status}");
        let wrong = Request::post("/send/start")
            .header("tailscale-user-login", LOGIN)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-atlas-csrf", "wrong")
            .header(
                header::COOKIE,
                format!("{}={}", crate::auth::COOKIE, hub.cookie),
            )
            .body(Body::from("{}"))
            .unwrap();
        assert_eq!(call(&hub.app, wrong).await.0, StatusCode::FORBIDDEN);
        assert!(hub.writer().releases.latest().is_none());
    }

    #[tokio::test]
    async fn a_change_is_pushed_signed_and_sent_once() {
        let hub = SendHub::new();
        let (_, home) = call(&hub.app, hub.get("/")).await;
        assert!(
            home.contains("Nothing has been sent to your machines yet"),
            "{home}"
        );

        // A saved but unpushed change in each store repo.
        commit(&hub.knowledge, "memory.md");
        commit(&hub.public, "skill.md");
        let public_upstream = git(&hub.public, &["rev-parse", "@{upstream}"]);
        let (status, body) = call(
            &hub.app,
            hub.post(
                "/send/start",
                r#"{"next":"/memory/fact?saved=abc&sent=1"}"#,
                true,
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let opts: serde_json::Value = serde_json::from_str(&body).unwrap();

        // Knowledge was pushed to its remote; public skills never are.
        let head = git(&hub.knowledge, &["rev-parse", "HEAD"]);
        let bare = hub.home.join("Knowledge.git");
        assert_eq!(git(&bare, &["rev-parse", "main"]), head);
        assert_eq!(
            git(&hub.home.join("skills.git"), &["rev-parse", "main"]),
            public_upstream
        );
        assert!(
            opts["note"]
                .as_str()
                .unwrap()
                .contains("Public skills reach your machines after you push them"),
            "{body}"
        );

        // The challenge is the digest of exactly the bytes that will be signed.
        let bytes = hub.pending_bytes();
        assert_eq!(
            opts["challenge"].as_str().unwrap(),
            atlas_signer::b64url(&Sha256::digest(&bytes))
        );
        assert_eq!(opts["rpId"], "hub.test");
        assert_eq!(opts["userVerification"], "required");
        assert_eq!(
            opts["allowCredentials"][0]["id"],
            atlas_signer::b64url(CRED_ID)
        );
        assert!(opts["summary"].as_str().unwrap().starts_with("Update 1"));
        let manifest: Manifest = serde_json::from_slice(&bytes).unwrap();
        let ids: Vec<_> = manifest.sources.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, vec![SourceId::Knowledge, SourceId::PublicSkills]);
        assert_eq!(manifest.sources[0].commit.as_str(), head);
        assert_eq!(manifest.sources[1].commit.as_str(), public_upstream);

        // A forged signature is refused and nothing is stored.
        let forged = assertion(b"some other digest");
        let (status, _) = call(&hub.app, hub.post("/send/finish", &forged, true)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(hub.writer().releases.latest().is_none());

        // Start again (the refused try used the pending update up) and sign it properly.
        let (status, body) = call(&hub.app, hub.post("/send/start", "{}", true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let digest_now: [u8; 32] = Sha256::digest(hub.pending_bytes()).into();
        let (status, body) = call(
            &hub.app,
            hub.post("/send/finish", &assertion(&digest_now), true),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body.contains("/memory/fact?saved=abc&sent=1"), "{body}");
        let sent = hub.writer().releases.latest().unwrap();
        assert_eq!(sent.number, 1);

        // Nothing new: no tap asked for.
        let (status, body) = call(&hub.app, hub.post("/send/start", "{}", true)).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let nothing: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(nothing["nothing"], true, "{body}");
        assert_eq!(nothing["next"], "/settings#updates");

        // Settings lists the update; the home page counts what's waiting.
        let (_, settings) = call(&hub.app, hub.get("/settings")).await;
        assert!(
            settings.contains("Update 1 · ") && settings.contains(" · Laptop"),
            "{settings}"
        );
        assert!(!settings.to_lowercase().contains("release"), "{settings}");
        commit(&hub.knowledge, "another.md");
        let (_, home) = call(&hub.app, hub.get("/")).await;
        // Only the new Knowledge commit: the public skill commit isn't pushed, so Send
        // couldn't deliver it and it isn't counted.
        assert!(
            home.contains("1 change isn&#39;t on your machines yet"),
            "{home}"
        );
        assert!(home.contains(r#"data-action="send""#));
    }

    #[tokio::test]
    async fn the_old_releases_page_moved_to_settings() {
        let hub = SendHub::new();
        let res = hub.app.clone().oneshot(hub.get("/releases")).await.unwrap();
        assert!(res.status().is_redirection());
        assert_eq!(res.headers()[header::LOCATION], "/settings#updates");
        let (_, home) = call(&hub.app, hub.get("/")).await;
        assert!(
            !home.contains("/releases") && !home.contains("Releases"),
            "nav item gone"
        );
    }

    #[tokio::test]
    async fn installing_over_ssh_needs_a_session_and_a_plain_address() {
        let hub = SendHub::new();
        let form = |body: String, signed_in: bool| {
            let mut r = Request::post("/machines/pair/ssh")
                .header("tailscale-user-login", LOGIN)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
            if signed_in {
                r = r.header(
                    header::COOKIE,
                    format!("{}={}", crate::auth::COOKIE, hub.cookie),
                );
            }
            r.body(Body::from(body)).unwrap()
        };
        let body = format!("csrf={}&target=laptop", hub.csrf);
        let (status, _) = call(&hub.app, form(body, false)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _) = call(&hub.app, form("csrf=wrong&target=laptop".into(), true)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let body = format!("csrf={}&target=-oProxyCommand%3Dx", hub.csrf);
        let (status, page) = call(&hub.app, form(body, true)).await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("isn&#39;t an SSH address"), "{page}");
        assert!(page.contains("marginalia warn-note"));
        assert!(page.contains("value=\"-oProxyCommand=x\""), "keeps what was typed");
        // Nothing ran, so no pairing command (and no code) was issued.
        assert!(!page.contains("sh -s --"));

        // The page offers the form and, once a code exists, the line for another computer.
        let (_, page) = call(&hub.app, hub.get("/machines")).await;
        assert!(page.contains("action=\"/machines/pair/ssh\""));
        let pair = Request::post("/machines/pair")
            .header("tailscale-user-login", LOGIN)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(
                header::COOKIE,
                format!("{}={}", crate::auth::COOKIE, hub.cookie),
            )
            .body(Body::from(format!("csrf={}", hub.csrf)))
            .unwrap();
        let (status, page) = call(&hub.app, pair).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains(&format!(
                "ssh you@machine 'curl -fsSL {ORIGIN}/install | sh -s -- "
            )),
            "{page}"
        );
    }
}
