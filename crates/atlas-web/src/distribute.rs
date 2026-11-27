//! Phase 3 on the hub: releases signed by the owner's passkey, pairing machines, and the
//! small API their clients use (decision 0019).

use std::sync::Arc;

use askama::Template;
use atlas_core::distribution::{PairRequest, PairResponse, Report};
use atlas_core::kernel::Scope;
use atlas_core::release::{Manifest, OwnerKey, Signature, SignedRelease};
use axum::Json;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::edit::{CsrfForm, Writer};
use crate::release::{self, Skipped};
use crate::views::Page;
use crate::{AppError, AppState};

/// How long a prepared release waits for its signature.
const PENDING_TTL: std::time::Duration = std::time::Duration::from_mins(10);
/// How long a pairing code works.
const PAIRING_TTL: std::time::Duration = std::time::Duration::from_mins(15);

/// A release built and shown to the owner, waiting for the passkey tap.
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

fn short(sha: &str) -> String {
    sha.chars().take(10).collect()
}

// ---------------------------------------------------------------- releases page

#[derive(Template)]
#[template(path = "releases.html")]
struct ReleasesPage {
    page: Page,
    rows: Vec<ReleaseRow>,
    review: Option<Review>,
    next: u64,
}

struct ReleaseRow {
    number: u64,
    created: String,
    signed_with: String,
    sources: usize,
}

/// A prepared release, shown before signing.
struct Review {
    number: u64,
    changes: Vec<SourceChange>,
    skipped: Vec<Skipped>,
    keys: Vec<String>,
    unchanged: bool,
}

struct SourceChange {
    name: String,
    from: String,
    to: String,
    changed: bool,
}

fn source_name(id: &atlas_core::release::SourceId) -> String {
    use atlas_core::release::SourceId;
    match id {
        SourceId::Knowledge => "Knowledge".to_owned(),
        SourceId::PublicSkills => "Public skills".to_owned(),
        SourceId::Project(p) => p.to_string(),
    }
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

fn day(secs: u64) -> String {
    crate::edit::day(secs)
}

async fn releases_view(
    s: &AppState,
    headers: &HeaderMap,
    review: Option<Review>,
) -> Result<Response, AppError> {
    let w = writer(s)?;
    let snap = s.snapshot().await?;
    let mut page = s.page(
        headers,
        "/releases",
        "Releases",
        &snap,
        &Scope::Global,
        false,
    );
    let numbers = w.releases.numbers();
    page.totals = format!("{} signed", numbers.len());
    let names: std::collections::HashMap<String, String> = w
        .signer
        .owner_keys()
        .into_iter()
        .map(|k| (k.credential_id, k.name))
        .collect();
    let rows = numbers
        .iter()
        .rev()
        .filter_map(|n| w.releases.get(*n))
        .map(|r| {
            let m = decode_manifest(&r);
            ReleaseRow {
                number: r.number,
                created: m.as_ref().map_or_else(String::new, |m| day(m.created)),
                signed_with: names
                    .get(&r.signature.credential_id)
                    .cloned()
                    .unwrap_or_else(|| "a passkey".to_owned()),
                sources: m.map_or(0, |m| m.sources.len()),
            }
        })
        .collect();
    let next = numbers.last().map_or(1, |n| n + 1);
    Ok(Html(
        ReleasesPage {
            page,
            rows,
            review,
            next,
        }
        .render()?,
    )
    .into_response())
}

pub(crate) async fn releases_page(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    releases_view(&s, &headers, None).await
}

/// Builds the next release from the repos as they stand and shows what changed.
pub(crate) async fn release_prepare(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<CsrfForm>,
) -> Result<Response, AppError> {
    let w = Arc::clone(writer(&s)?);
    if !w.authorised(&headers, f.csrf()) {
        return Ok(forbidden("Sign in again to prepare a release.\n"));
    }
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
    let key_names: Vec<String> = keys.iter().map(|k| k.name.clone()).collect();
    let (manifest, skipped) = tokio::task::spawn_blocking(move || {
        release::build(&sources, number, &origin, keys, crate::clients::now())
    })
    .await
    .map_err(|_| AppError::Internal)?;
    let changes: Vec<SourceChange> = manifest
        .sources
        .iter()
        .map(|src| {
            let before = previous
                .as_ref()
                .and_then(|p| p.source(&src.id))
                .map(|p| p.commit.as_str().to_owned());
            SourceChange {
                name: source_name(&src.id),
                changed: before.as_deref() != Some(src.commit.as_str()),
                from: before.map_or_else(|| "new".to_owned(), |b| short(&b)),
                to: short(src.commit.as_str()),
            }
        })
        .collect();
    let unchanged = previous
        .as_ref()
        .is_some_and(|p| p.sources == manifest.sources && p.owner_keys == manifest.owner_keys);
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
    let review = Review {
        number,
        changes,
        skipped,
        keys: key_names,
        unchanged,
    };
    releases_view(&s, &headers, Some(review)).await
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
}

#[derive(Serialize)]
struct AllowedKey {
    id: String,
    #[serde(rename = "type")]
    kind: &'static str,
}

/// The passkey request for the pending release: its challenge is the manifest digest.
pub(crate) async fn release_sign_start(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    let csrf = headers
        .get("x-atlas-csrf")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !w.authorised(&headers, csrf) {
        return Ok(forbidden("Sign in again to sign a release.\n"));
    }
    let pending = w
        .pending
        .lock()
        .ok()
        .and_then(|p| p.clone())
        .filter(|p| p.expires > std::time::Instant::now());
    let Some(p) = pending else {
        return Ok(forbidden("Prepare the release again; that one expired.\n"));
    };
    let rp_id = atlas_signer::Url::parse(w.origin())
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
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
        challenge: atlas_signer::b64url(&p.digest),
        rp_id,
        allow,
        user_verification: "required",
        timeout: 120_000,
    })
    .into_response())
}

#[derive(Deserialize)]
pub struct SignFinish {
    credential: Signature,
}

#[derive(Serialize)]
struct Done {
    next: String,
}

/// Checks the passkey's signature over the pending release exactly as a client will, then
/// stores the release.
pub(crate) async fn release_sign_finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SignFinish>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    let csrf = headers
        .get("x-atlas-csrf")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    if !w.authorised(&headers, csrf) {
        return Ok(forbidden("Sign in again to sign a release.\n"));
    }
    let pending = w
        .pending
        .lock()
        .ok()
        .and_then(|mut p| p.take())
        .filter(|p| p.expires > std::time::Instant::now());
    let Some(p) = pending else {
        return Ok(forbidden("Prepare the release again; that one expired.\n"));
    };
    let keys: Vec<atlas_verify::OwnerKey> = w
        .signer
        .owner_keys()
        .iter()
        .filter_map(verify_key)
        .collect();
    let rp_id = atlas_signer::Url::parse(w.origin())
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default();
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
        tracing::warn!("release {} signature refused: {e:?}", p.number);
        return Ok(forbidden(
            "That signature didn't check out, so the release wasn't saved.\n",
        ));
    }
    let release = SignedRelease {
        number: p.number,
        manifest: atlas_signer::b64url(&p.bytes),
        signature: req.credential,
    };
    w.releases.save(&release).map_err(AppError::Bad)?;
    tracing::warn!("release {} signed and published", p.number);
    Ok(Json(Done {
        next: "/releases".into(),
    })
    .into_response())
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
    let code = atlas_signer::setup::issue_in(
        &w.state_dir,
        atlas_signer::setup::PAIRING_CODES,
        PAIRING_TTL,
    )
    .map_err(|e| AppError::Bad(e.to_string()))?;
    crate::machines_page(
        &s,
        &headers,
        Some(format!(
            "curl -fsSL {}/install | sh -s -- {code}",
            w.origin()
        )),
    )
    .await
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
}
