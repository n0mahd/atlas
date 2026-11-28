//! Phase 3 on the hub: updates for the machines (signed releases underneath) sent with
//! the owner's passkey, pairing machines, and the small API their clients use
//! (decisions 0019, 0022).

use std::sync::Arc;

use askama::Template as _;
use atlas_core::distribution::{
    AddMachineRequest, ClientSummary, MachineSummary, MachinesListResponse, PairCodeResponse,
    PairRequest, PairResponse, RemoveMachineResponse, Report,
};
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

/// Updates one session may have waiting at once.
const PENDING_PER_SESSION: usize = 4;

/// An update built for the owner, waiting for the passkey tap. Its manifest bytes are
/// fixed when it's built: the check page shows what they pin and the tap signs them, so
/// nothing committed afterwards can slip in.
#[derive(Debug, Clone)]
pub struct Pending {
    pub number: u64,
    pub bytes: Vec<u8>,
    pub digest: [u8; 32],
    /// The CSRF token of the session that built it: only that session may sign it.
    session: String,
    /// What it changes, read from exactly the commits it pins.
    pub(crate) review: crate::review::Review,
    /// Whether its check page was served (needed before signing when it has hooks).
    shown: bool,
    /// Where to go once sent, and on "Not now".
    next: String,
    back: String,
    expires: std::time::Instant,
}

impl Pending {
    fn usable_by(&self, session: &str) -> bool {
        self.expires > std::time::Instant::now() && auth::same_secret(&self.session, session)
    }
}

/// The session's CSRF token, which identifies it.
fn session_of(w: &Writer, headers: &HeaderMap) -> Option<String> {
    w.auth.session(headers).map(|s| s.csrf)
}

/// Keeps `p` under a fresh id; returns the id.
fn keep(w: &Writer, p: Pending) -> String {
    let id = auth::token();
    if let Ok(mut map) = w.pending.lock() {
        let now = std::time::Instant::now();
        map.retain(|_, q| q.expires > now);
        let mine: Vec<(String, std::time::Instant)> = map
            .iter()
            .filter(|(_, q)| auth::same_secret(&q.session, &p.session))
            .map(|(k, q)| (k.clone(), q.expires))
            .collect();
        if mine.len() >= PENDING_PER_SESSION
            && let Some((oldest, _)) = mine.iter().min_by_key(|(_, e)| *e)
        {
            map.remove(oldest);
        }
        map.insert(id.clone(), p);
    }
    id
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
    /// The update being signed; `/send/finish` names it.
    pending: String,
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

/// What Send says when the machines already have everything.
pub(crate) const NOTHING_NEW: &str = "Your machines already have everything.";

#[derive(Serialize)]
struct NothingToSend {
    nothing: bool,
    /// [`NOTHING_NEW`], for the page to show in place.
    message: &'static str,
    next: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<String>,
}

/// The update has hooks to look at first: the page to show.
#[derive(Serialize)]
struct LookFirst {
    review: String,
}

#[derive(Deserialize)]
pub struct SendStart {
    /// Where to go once sent.
    #[serde(default)]
    next: String,
    /// Where "Not now" goes.
    #[serde(default)]
    back: String,
    /// An update already built (and shown); sign exactly that one.
    #[serde(default)]
    pending: String,
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

/// Pushes Knowledge if the remote lacks some of its commits, then builds the update and
/// reads what it changes from the commits it pins. Returns them with notes for the owner,
/// such as public skills waiting for a push.
fn prepare(
    writer: &Writer,
    sources: &atlas_fs::Sources,
    previous: Option<&Manifest>,
    number: u64,
    origin: &str,
    keys: Vec<OwnerKey>,
) -> Result<(Manifest, crate::review::Review, Vec<String>), String> {
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
    let (manifest, skipped) = release::build(sources, number, origin, keys, crate::clients::now())?;
    notes.extend(
        skipped
            .iter()
            .map(|k| format!("{} {}.", k.source, k.reason)),
    );
    let review = crate::review::review(sources, previous, &manifest);
    Ok((manifest, review, notes))
}

/// What building an update came to.
enum Built {
    /// The machines already have all of it.
    Nothing(Option<String>),
    /// Kept under this id.
    Kept {
        id: String,
        look: bool,
        note: Option<String>,
    },
}

/// Builds the next update from the commits machines can fetch now and keeps it, bytes
/// fixed, for this session to sign.
async fn build_pending(
    s: &AppState,
    w: &Arc<Writer>,
    session: String,
    next: String,
    back: String,
) -> Result<Built, AppError> {
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
    let writer = Arc::clone(w);
    let before = previous.clone();
    let built = tokio::task::spawn_blocking(move || {
        prepare(&writer, &sources, before.as_ref(), number, &origin, keys)
    })
    .await
    .map_err(|_| AppError::Internal)?;
    s.invalidate().await;
    let (manifest, review, notes) = built.map_err(AppError::Bad)?;
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
        return Ok(Built::Nothing(note));
    }
    let bytes = serde_json::to_vec(&manifest).map_err(|_| AppError::Internal)?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    let look = review.needs_look();
    let id = keep(
        w,
        Pending {
            number,
            bytes,
            digest,
            session,
            review,
            shown: false,
            next,
            back,
            expires: std::time::Instant::now() + PENDING_TTL,
        },
    );
    Ok(Built::Kept { id, look, note })
}

fn review_href(id: &str) -> String {
    format!("/send/review/{id}")
}

/// Starts sending. Without `pending`: pushes Knowledge if it has commits the remote lacks
/// (never public skills) and builds the next update; when it adds or changes a hook the
/// answer is the check page to show, otherwise the passkey options straight away. With
/// `pending`: the passkey options for that update exactly as it was built (and shown).
/// Answers `nothing` when machines already have all of it.
pub(crate) async fn send_start(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SendStart>,
) -> Result<Response, AppError> {
    let w = Arc::clone(writer(&s)?);
    if !w.authorised(&headers, csrf_header(&headers)) {
        return Ok(forbidden("Sign in again to send changes.\n"));
    }
    let Some(session) = session_of(&w, &headers) else {
        return Ok(forbidden("Sign in again to send changes.\n"));
    };
    let (id, note) = if req.pending.is_empty() {
        let next = next_or_settings(&req.next);
        let back = next_or_settings(&req.back);
        match build_pending(&s, &w, session.clone(), next.clone(), back).await? {
            Built::Nothing(note) => {
                return Ok(Json(NothingToSend {
                    nothing: true,
                    message: NOTHING_NEW,
                    next,
                    note,
                })
                .into_response());
            }
            Built::Kept { id, look: true, .. } => {
                return Ok(Json(LookFirst {
                    review: review_href(&id),
                })
                .into_response());
            }
            Built::Kept { id, note, .. } => (id, note),
        }
    } else {
        (req.pending.clone(), None)
    };
    let found = w
        .pending
        .lock()
        .ok()
        .and_then(|map| map.get(&id).cloned())
        .filter(|p| p.usable_by(&session));
    let Some(p) = found else {
        return Ok(forbidden("That request expired. Send again.\n"));
    };
    if p.review.needs_look() && !p.shown {
        return Ok(Json(LookFirst {
            review: review_href(&id),
        })
        .into_response());
    }
    let previous = w.releases.latest().and_then(|r| decode_manifest(&r));
    let manifest: Manifest = serde_json::from_slice(&p.bytes).map_err(|_| AppError::Internal)?;
    let changed: Vec<String> = manifest
        .sources
        .iter()
        .filter(|src| previous.as_ref().and_then(|q| q.source(&src.id)) != Some(*src))
        .map(|src| release::label(&src.id))
        .collect();
    // What the tap signs, in things the owner can check: "Update 4 · 1 note changed".
    let summary = match p.review.changed.summary() {
        Some(counts) => format!("Update {} · {counts}", p.number),
        None if changed.is_empty() => format!("Update {} · passkeys", p.number),
        None => format!("Update {} · {}", p.number, changed.join(", ")),
    };
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
        rp_id: rp_id(&w),
        allow,
        user_verification: "required",
        timeout: 120_000,
        pending: id,
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
    /// The update `/send/start` handed out.
    #[serde(default)]
    pending: String,
}

#[derive(Serialize)]
struct Done {
    next: String,
}

/// Checks the passkey's signature over the named pending update exactly as a client will,
/// then stores those same bytes for the machines to fetch. Never rebuilds the update.
pub(crate) async fn send_finish(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<SendFinish>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, csrf_header(&headers)) {
        return Ok(forbidden("Sign in again to send changes.\n"));
    }
    let Some(session) = session_of(w, &headers) else {
        return Ok(forbidden("Sign in again to send changes.\n"));
    };
    // One try per built update: a refused signature uses it up.
    let pending = w
        .pending
        .lock()
        .ok()
        .and_then(|mut map| map.remove(&req.pending))
        .filter(|p| p.usable_by(&session))
        .filter(|p| !p.review.needs_look() || p.shown);
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
    if let Ok(mut last) = w.last_sent.lock() {
        *last = Some((p.number, p.review.others.clone()));
    }
    tracing::warn!("update {} signed and sent", p.number);
    let next = if req.next.is_empty() {
        p.next
    } else {
        next_or_settings(&req.next)
    };
    Ok(Json(Done { next }).into_response())
}

/// The "other things" line for the update just sent, for the notice after sending.
pub(crate) fn sent_others(s: &AppState) -> Option<crate::views::OthersView> {
    let w = s.writer.as_ref()?;
    let latest = w.releases.numbers().last().copied()?;
    let last = w.last_sent.lock().ok()?;
    let (number, others) = last.as_ref()?;
    if *number != latest {
        return None;
    }
    crate::views::OthersView::of(others)
}

#[derive(Deserialize)]
pub(crate) struct ReviewForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    next: String,
    #[serde(default)]
    back: String,
}

/// Send without the page's script: builds the update and opens its check page, which
/// works as a plain page (the passkey tap itself needs JavaScript).
pub(crate) async fn review_build(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ReviewForm>,
) -> Result<Response, AppError> {
    let w = Arc::clone(writer(&s)?);
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to send changes.\n"));
    }
    let Some(session) = session_of(&w, &headers) else {
        return Ok(forbidden("Sign in again to send changes.\n"));
    };
    let next = next_or_settings(&f.next);
    let back = next_or_settings(&f.back);
    match build_pending(&s, &w, session, next, back.clone()).await? {
        Built::Nothing(_) => Ok(Redirect::to("/settings?flash=nothing#updates").into_response()),
        Built::Kept { id, .. } => Ok(Redirect::to(&review_href(&id)).into_response()),
    }
}

/// The check page: each new or changed hook in full, read from exactly the commits the
/// update pins, with Send (the passkey tap) and Not now.
pub(crate) async fn review_page(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    let Some(session) = session_of(w, &headers) else {
        return Ok(Redirect::to("/signin?next=/settings").into_response());
    };
    let found = w.pending.lock().ok().and_then(|mut map| {
        let p = map.get_mut(&id).filter(|p| p.usable_by(&session))?;
        p.shown = true;
        Some(p.clone())
    });
    let Some(p) = found else {
        return Ok(Redirect::to("/settings#updates").into_response());
    };
    let snap = s.snapshot().await?;
    let mut page = s.page(
        &headers,
        "/settings",
        "Check before sending",
        &snap,
        &atlas_core::kernel::Scope::Global,
        false,
    );
    page.totals = format!("Update {}", p.number);
    let html = crate::SendReviewPage {
        page,
        id,
        next: p.next.clone(),
        back: p.back.clone(),
        hooks: p.review.hooks.clone(),
        keys: p.review.keys.clone(),
        trusted: p.review.trusted.clone(),
        others: crate::views::OthersView::of(&p.review.others),
    }
    .render()?;
    Ok(axum::response::Html(html).into_response())
}

/// The old Releases page lives in Settings now.
pub(crate) async fn releases_moved() -> Redirect {
    Redirect::to("/settings#updates")
}

// ---------------------------------------------------------------- pairing (owner side)

#[derive(Deserialize)]
pub(crate) struct PairForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    machine: String,
}

/// Issues a one-time pairing code and shows the command to run on the machine.
pub(crate) async fn pair_code(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<PairForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to pair a machine.\n"));
    }
    let code = issue_pairing_code(w)?;
    let pair_target = remote::Target::parse(&f.machine)
        .ok()
        .map(|t| t.to_string());
    crate::machines_page(
        &s,
        &headers,
        Some(remote::install_command(w.origin(), &code)),
        pair_target,
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
pub(crate) struct MachineNameForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    name: String,
}

/// Adds a machine name to the hub's tracked list.
pub(crate) async fn add_machine_ui(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<MachineNameForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to add a machine.\n"));
    }
    let trimmed = f.name.trim();
    if trimmed.eq_ignore_ascii_case(&s.hub) {
        let result = SshResult {
            ok: false,
            message: format!("{trimmed} is already this hub."),
            output: String::new(),
            target: String::new(),
        };
        return crate::machines_page(&s, &headers, None, None, Some(result)).await;
    }
    match w.machines.add(trimmed) {
        Ok(_) => Ok(Redirect::to("/machines").into_response()),
        Err(message) => {
            let result = SshResult {
                ok: false,
                message,
                output: String::new(),
                target: String::new(),
            };
            crate::machines_page(&s, &headers, None, None, Some(result)).await
        }
    }
}

/// Removes a tracked machine and unpairs any clients on it.
pub(crate) async fn forget_machine_ui(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<MachineNameForm>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if !w.authorised(&headers, &f.csrf) {
        return Ok(forbidden("Sign in again to remove a machine.\n"));
    }
    let name = f.name.trim();
    if !name.is_empty() && !name.eq_ignore_ascii_case(&s.hub) {
        w.machines.remove(name).map_err(AppError::Bad)?;
        w.clients.remove_machine(name).map_err(AppError::Bad)?;
    }
    Ok(Redirect::to("/machines").into_response())
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
            return crate::machines_page(&s, &headers, None, None, Some(result)).await;
        }
    };
    let code = issue_pairing_code(w)?;
    tracing::info!("installing atlasd on {target} over ssh");
    let (result, command, pair_target) = match remote::install_over_ssh(w.origin(), &code, &target)
        .await
    {
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
                Some(target.to_string()),
            )
        }
    };
    crate::machines_page(&s, &headers, command, pair_target, Some(result)).await
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

// ---------------------------------------------------------------- admin CLI API

fn unauthorised_cli(w: &Writer, headers: &HeaderMap) -> Option<Response> {
    if w.tokens.authenticate(headers).is_some() {
        None
    } else {
        Some((
            StatusCode::UNAUTHORIZED,
            "Invalid or missing CLI token. Create one in Settings → CLI tokens, then run `atlasd auth`.\n",
        )
            .into_response())
    }
}

pub(crate) async fn api_admin_machines(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if let Some(res) = unauthorised_cli(w, &headers) {
        return Ok(res);
    }
    let clients = w.clients.list();
    let latest_release = w.releases.numbers().last().copied();
    let mut names: Vec<String> = s
        .known_machines()
        .into_iter()
        .filter(|n| !n.eq_ignore_ascii_case(&s.hub))
        .collect();
    for c in &clients {
        if !c.machine.eq_ignore_ascii_case(&s.hub)
            && !names.iter().any(|n| n.eq_ignore_ascii_case(&c.machine))
        {
            names.push(c.machine.clone());
        }
    }
    let to_summary = |machine: &str, hub: bool| MachineSummary {
        name: machine.to_owned(),
        hub,
        clients: clients
            .iter()
            .filter(|c| c.machine.eq_ignore_ascii_case(machine))
            .map(|c| ClientSummary {
                id: c.id.clone(),
                user: c.user.clone(),
                paired: c.paired,
                last_seen: c.last_seen,
                release: c.report.as_ref().and_then(|r| r.release),
                problems: c
                    .report
                    .as_ref()
                    .map_or_else(Vec::new, |r| r.problems.clone()),
            })
            .collect(),
    };
    let mut machines = vec![to_summary(&s.hub, true)];
    machines.extend(names.iter().map(|n| to_summary(n, false)));
    Ok(Json(MachinesListResponse {
        hub: s.hub.to_string(),
        origin: w.origin().to_owned(),
        latest_release,
        machines,
    })
    .into_response())
}

pub(crate) async fn api_admin_pair_code(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if let Some(res) = unauthorised_cli(w, &headers) {
        return Ok(res);
    }
    let code = issue_pairing_code(w)?;
    let origin = w.origin().to_owned();
    let command = remote::install_command(&origin, &code);
    Ok(Json(PairCodeResponse {
        origin,
        code,
        command,
        ttl_seconds: PAIRING_TTL.as_secs(),
    })
    .into_response())
}

pub(crate) async fn api_admin_add_machine(
    State(s): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<AddMachineRequest>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if let Some(res) = unauthorised_cli(w, &headers) {
        return Ok(res);
    }
    let trimmed = req.name.trim();
    if trimmed.eq_ignore_ascii_case(&s.hub) {
        return Err(AppError::Bad(format!("{trimmed} is already this hub.")));
    }
    let name = w.machines.add(trimmed).map_err(AppError::Bad)?;
    Ok((StatusCode::CREATED, Json(AddMachineRequest { name })).into_response())
}

pub(crate) async fn api_admin_remove_machine(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(target): Path<String>,
) -> Result<Response, AppError> {
    let w = writer(&s)?;
    if let Some(res) = unauthorised_cli(w, &headers) {
        return Ok(res);
    }
    let target = target.trim();
    if target.is_empty() {
        return Err(AppError::Bad("Give a machine name or client id.".into()));
    }
    if w.clients.list().iter().any(|c| c.id == target) {
        w.clients.remove(target).map_err(AppError::Bad)?;
        return Ok(Json(RemoveMachineResponse {
            removed_machine: false,
            removed_clients: 1,
        })
        .into_response());
    }
    if target.eq_ignore_ascii_case(&s.hub) {
        return Err(AppError::Bad(format!(
            "{target} is this hub; unpair a specific client id if needed."
        )));
    }
    let removed_machine = w.machines.remove(target).map_err(AppError::Bad)?;
    let removed_clients = w.clients.remove_machine(target).map_err(AppError::Bad)?;
    if !removed_machine && removed_clients == 0 {
        return Ok((
            StatusCode::NOT_FOUND,
            format!("No machine or paired client named \"{target}\".\n"),
        )
            .into_response());
    }
    Ok(Json(RemoveMachineResponse {
        removed_machine,
        removed_clients,
    })
    .into_response())
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

    /// A passkey, as an authenticator holds it.
    fn key_from(seed: u8) -> p256::ecdsa::SigningKey {
        p256::ecdsa::SigningKey::from_slice(&[seed; 32]).unwrap()
    }

    /// The owner's passkey.
    fn owner_key() -> p256::ecdsa::SigningKey {
        key_from(7)
    }

    const CRED_ID: &[u8] = &[1, 2, 3, 4];

    /// Writes the hub's passkey store with these keys: (name, seed, credential id).
    fn write_keys(state: &Path, keys: &[(&str, u8, &[u8])]) {
        let b64 = atlas_signer::b64url;
        let entries: Vec<String> = keys
            .iter()
            .map(|(name, seed, cred)| {
                let point = key_from(*seed).verifying_key().to_sec1_point(false);
                let raw = point.as_bytes();
                let passkey = format!(
                    r#"{{"cred":{{"cred_id":"{}","cred":{{"type_":"ES256","key":{{"EC_EC2":{{"curve":"SECP256R1","x":"{}","y":"{}"}}}}}},"counter":0,"transports":null,"user_verified":true,"backup_eligible":false,"backup_state":false,"registration_policy":"required","extensions":{{"cred_protect":"NotRequested","hmac_create_secret":"NotRequested","appid":"NotRequested","cred_props":"NotRequested"}},"attestation":{{"data":"None","metadata":"None"}},"attestation_format":"none"}}}}"#,
                    b64(cred),
                    b64(&raw[1..33]),
                    b64(&raw[33..65])
                );
                format!(r#"{{"name":"{name}","added":0,"last_used":null,"passkey":{passkey}}}"#)
            })
            .collect();
        let store = format!(
            r#"{{"user_id":"00000000-0000-4000-8000-000000000001","keys":[{}]}}"#,
            entries.join(",")
        );
        std::fs::create_dir_all(state).unwrap();
        std::fs::write(state.join("passkeys.json"), store).unwrap();
    }

    /// Writes the hub's passkey store with the owner's one key, named Laptop.
    fn enrol(state: &Path) {
        write_keys(state, &[("Laptop", 7, CRED_ID)]);
    }

    /// Signs `challenge` for pending update `pending` exactly as a browser's passkey
    /// prompt would.
    fn assertion(challenge: &[u8], pending: &str) -> String {
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
            r#"{{"next":"/memory/fact?saved=abc&sent=1","pending":"{pending}","credential":{{"credential_id":"{}","authenticator_data":"{}","client_data_json":"{}","signature":"{}"}}}}"#,
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

        fn pending_bytes(&self, id: &str) -> Vec<u8> {
            self.writer().pending.lock().unwrap()[id].bytes.clone()
        }

        /// Starts a send; `pending` names an update already built.
        async fn start(&self, pending: &str) -> serde_json::Value {
            let body = format!(r#"{{"pending":"{pending}"}}"#);
            let (status, body) = call(&self.app, self.post("/send/start", &body, true)).await;
            assert_eq!(status, StatusCode::OK, "{body}");
            serde_json::from_str(&body).unwrap()
        }

        /// Signs pending update `id` with the owner's passkey.
        async fn sign(&self, id: &str) -> (StatusCode, String) {
            let digest: [u8; 32] = Sha256::digest(self.pending_bytes(id)).into();
            call(
                &self.app,
                self.post("/send/finish", &assertion(&digest, id), true),
            )
            .await
        }

        /// Sends everything waiting, looking at the check page first when there is one.
        async fn send_all(&self) {
            let opts = self.start("").await;
            let id = match opts["review"].as_str() {
                Some(href) => {
                    let (status, _) = call(&self.app, self.get(href)).await;
                    assert_eq!(status, StatusCode::OK);
                    let id = href.trim_start_matches("/send/review/").to_owned();
                    let opts = self.start(&id).await;
                    assert!(opts["challenge"].is_string(), "{opts}");
                    id
                }
                None => opts["pending"].as_str().unwrap().to_owned(),
            };
            let (status, body) = self.sign(&id).await;
            assert_eq!(status, StatusCode::OK, "{body}");
        }

        /// The hub restarted: same state folder and repos, a fresh writer.
        fn reopen(self) -> Self {
            let state_dir = self.home.join("state");
            let writer =
                crate::Writer::open(ORIGIN, &state_dir, std::time::Duration::ZERO).unwrap();
            let sources = atlas_fs::Sources {
                home: self.home.clone(),
                knowledge: self.knowledge.clone(),
                public_skills: Some(self.public.clone()),
                projects: Vec::new(),
            };
            let state = crate::AppState::new(sources, "hub", Vec::new(), Some(writer));
            let app = crate::router(state.clone(), crate::Access::new(vec![LOGIN.into()]));
            Self { state, app, ..self }
        }

        fn latest_manifest(&self) -> Manifest {
            super::decode_manifest(&self.writer().releases.latest().unwrap()).unwrap()
        }
    }

    /// Adds hook `name` (before Bash runs, running `run.sh`) to the repo and commits it.
    fn add_hook(dir: &Path, name: &str, script: &str) {
        let hook = dir.join("hooks").join(name);
        std::fs::create_dir_all(&hook).unwrap();
        std::fs::write(
            hook.join("hook.toml"),
            format!(
                "description = \"Checks {name}\"\nevent = \"pre-tool-use\"\nmatcher = \"Bash\"\nrun = \"run.sh\"\n"
            ),
        )
        .unwrap();
        std::fs::write(hook.join("run.sh"), script).unwrap();
        set_exec(&hook.join("run.sh"), true);
        git(dir, &["add", "-A"]);
        git(dir, &["commit", "-q", "-m", &format!("Add {name}")]);
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
        let id = opts["pending"].as_str().unwrap().to_owned();
        let bytes = hub.pending_bytes(&id);
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
        let forged = assertion(b"some other digest", &id);
        let (status, _) = call(&hub.app, hub.post("/send/finish", &forged, true)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(hub.writer().releases.latest().is_none());

        // Start again (the refused try used the pending update up) and sign it properly.
        let again = hub.start("").await;
        let id = again["pending"].as_str().unwrap().to_owned();
        let (status, body) = hub.sign(&id).await;
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
    async fn the_machine_list_refreshes_without_the_pairing_forms() {
        let hub = SendHub::new();
        let body = format!("csrf={}&name=workstation", hub.csrf);
        call(&hub.app, post_form(&hub, "/machines/add", body)).await;
        let (status, page) = call(&hub.app, hub.get("/machines")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains(
            r#"<div id="machine-list" class="machine-list" hx-get="/machines/list" hx-trigger="every 10s, visibilitychange from:document" hx-swap="outerHTML" data-poll>"#
        ));
        assert!(page.contains("action=\"/machines/pair/ssh\""));

        let (status, list) = call(&hub.app, hub.get("/machines/list")).await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            list.trim_start().starts_with("<div id=\"machine-list\""),
            "{list}"
        );
        assert!(list.contains("class=\"ledger"), "{list}");
        assert!(list.contains("<h2>hub</h2>"), "{list}");
        // The unpaired machine's own row keeps its Pair, Install and remove buttons.
        assert!(list.contains("<h2>workstation</h2>"), "{list}");
        assert!(
            list.contains("name=\"machine\" value=\"workstation\""),
            "{list}"
        );
        assert!(list.contains("action=\"/machines/forget\""), "{list}");
        // The forms at the top of the page (typing, copying) are never replaced.
        for absent in [
            "Pair a machine",
            "name=\"target\" placeholder",
            "action=\"/machines/add\"",
            "<html",
            "<nav",
        ] {
            assert!(!list.contains(absent), "{absent} in {list}");
        }
        // The same ledgers as the page, from the one partial.
        let start = page.find("<div id=\"machine-list\"").unwrap();
        assert!(page[start..].starts_with(list.trim()));
    }

    #[tokio::test]
    async fn the_install_script_starts_a_first_run_at_once() {
        let hub = SendHub::new();
        let dist = hub.home.join("state/dist");
        std::fs::create_dir_all(&dist).unwrap();
        std::fs::write(dist.join("atlasd-linux-x86_64"), b"build").unwrap();
        let (status, script) = call(&hub.app, hub.get("/install")).await;
        assert_eq!(status, StatusCode::OK);
        let enable = script
            .find("systemctl --user enable --now atlasd.timer\n")
            .expect("timer enabled");
        let start = script
            .find("\nsystemctl --user start --no-block atlasd.service\n")
            .expect("first run started");
        assert!(enable < start, "{script}");
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
        assert!(
            page.contains("value=\"-oProxyCommand=x\""),
            "keeps what was typed"
        );
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

    fn post_form(hub: &SendHub, path: &str, body: String) -> Request<Body> {
        Request::post(path)
            .header("tailscale-user-login", LOGIN)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(
                header::COOKIE,
                format!("{}={}", crate::auth::COOKIE, hub.cookie),
            )
            .body(Body::from(body))
            .unwrap()
    }

    /// Marks a file executable or not.
    fn set_exec(path: &Path, exec: bool) {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = if exec { 0o755 } else { 0o644 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// The check page `opts` points at, read as the owner sees it.
    async fn review_page(hub: &SendHub, opts: &serde_json::Value) -> (String, String) {
        let href = opts["review"]
            .as_str()
            .unwrap_or_else(|| panic!("a page to look at first: {opts}"));
        let (status, page) = call(&hub.app, hub.get(href)).await;
        assert_eq!(status, StatusCode::OK, "{page}");
        (href.trim_start_matches("/send/review/").to_owned(), page)
    }

    #[tokio::test]
    async fn an_unchanged_hooks_send_is_one_tap() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        hub.send_all().await;
        std::fs::create_dir_all(hub.knowledge.join("memory")).unwrap();
        commit(&hub.knowledge, "memory/a.md");
        let opts = hub.start("").await;
        assert!(opts["review"].is_null(), "no page: {opts}");
        assert!(opts["challenge"].is_string(), "{opts}");
        assert_eq!(opts["summary"], "Update 2 · 1 note changed");
        let (status, body) = hub.sign(opts["pending"].as_str().unwrap()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        // The notice after sending names what else changed outside Atlas.
        let (_, page) = call(&hub.app, hub.get("/?sent=1")).await;
        assert!(page.contains("1 note changed outside Atlas"), "{page}");
    }

    #[tokio::test]
    async fn first_send_lists_every_hook() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        add_hook(&hub.knowledge, "notify", "echo notify\n");
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        for text in [
            "guard",
            "notify",
            "echo guard",
            "echo notify",
            "Before a tool runs · Bash",
            "Laptop",
            "Trusted by your machines",
            ">Not now<",
        ] {
            assert!(page.contains(text), "{text}: {page}");
        }
        let main = page[page.find("<main").unwrap()..].to_lowercase();
        for word in ["manifest", "release", "tree", "executable"] {
            assert!(!main.contains(word), "{word}: {main}");
        }
        assert!(hub.writer().releases.latest().is_none(), "nothing sent yet");
    }

    #[tokio::test]
    async fn a_deleted_hook_needs_no_look() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        hub.send_all().await;
        std::fs::remove_dir_all(hub.knowledge.join("hooks/guard")).unwrap();
        git(&hub.knowledge, &["add", "-A"]);
        git(&hub.knowledge, &["commit", "-q", "-m", "Remove guard"]);
        let opts = hub.start("").await;
        assert!(opts["review"].is_null(), "{opts}");
        assert!(opts["challenge"].is_string(), "{opts}");
    }

    #[tokio::test]
    async fn a_changed_hook_shows_its_diff() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        hub.send_all().await;
        std::fs::write(hub.knowledge.join("hooks/guard/run.sh"), "echo louder\n").unwrap();
        git(&hub.knowledge, &["commit", "-q", "-am", "Louder"]);
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        assert!(
            page.contains(r#"<span class="d-del">-echo guard</span>"#),
            "{page}"
        );
        assert!(
            page.contains(r#"<span class="d-add">+echo louder</span>"#),
            "{page}"
        );
        assert!(page.contains("Changed hook"), "{page}");
        assert!(
            !page.contains("Trusted by your machines"),
            "only on the first"
        );
    }

    #[tokio::test]
    async fn a_hook_whose_script_isnt_executable_says_so() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        set_exec(&hub.knowledge.join("hooks/guard/run.sh"), false);
        git(&hub.knowledge, &["commit", "-q", "-am", "Plain"]);
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        assert!(
            page.contains("isn't marked as runnable, so your machines will skip this hook"),
            "{page}"
        );
        set_exec(&hub.knowledge.join("hooks/guard/run.sh"), true);
        git(&hub.knowledge, &["commit", "-q", "-am", "Runnable"]);
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        assert!(!page.contains("marked as runnable"), "{page}");
    }

    #[tokio::test]
    async fn the_update_signed_is_the_one_shown() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        let opts = hub.start("").await;
        let id = opts["review"]
            .as_str()
            .unwrap()
            .trim_start_matches("/send/review/")
            .to_owned();

        // It can't be signed before its page is shown.
        let early = hub.start(&id).await;
        assert!(early["challenge"].is_null(), "{early}");
        assert!(early["review"].is_string(), "{early}");
        let digest: [u8; 32] = Sha256::digest(hub.pending_bytes(&id)).into();
        let (status, _) = call(
            &hub.app,
            hub.post("/send/finish", &assertion(&digest, &id), true),
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);

        let opts = hub.start("").await;
        let (id, page) = review_page(&hub, &opts).await;
        assert!(page.contains("echo guard") && !page.contains("echo late"));
        let shown: Manifest = serde_json::from_slice(&hub.pending_bytes(&id)).unwrap();
        let shown_commit = shown.sources[0].commit.as_str().to_owned();

        // A new hook is committed and pushed after the page was built.
        add_hook(&hub.knowledge, "late", "echo late\n");
        git(&hub.knowledge, &["push", "-q"]);
        let late_head = git(&hub.knowledge, &["rev-parse", "HEAD"]);

        let opts = hub.start(&id).await;
        assert_eq!(
            opts["challenge"].as_str().unwrap(),
            atlas_signer::b64url(&Sha256::digest(hub.pending_bytes(&id)))
        );
        let (status, body) = hub.sign(&id).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let stored = hub.latest_manifest();
        assert_eq!(stored.sources[0].commit.as_str(), shown_commit);
        assert_ne!(shown_commit, late_head);
        let hooks = git(
            &hub.knowledge,
            &["ls-tree", "--name-only", &shown_commit, "hooks/"],
        );
        assert!(
            hooks.contains("hooks/guard") && !hooks.contains("hooks/late"),
            "{hooks}"
        );

        // The next send asks about the late hook.
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        assert!(
            page.contains("echo late") && !page.contains("echo guard"),
            "{page}"
        );
    }

    #[tokio::test]
    async fn a_new_passkey_needs_a_look_and_is_what_gets_signed() {
        let hub = SendHub::new();
        hub.send_all().await;
        write_keys(
            &hub.home.join("state"),
            &[("Laptop", 7, CRED_ID), ("Phone", 9, &[5, 6, 7, 8])],
        );
        let hub = hub.reopen();
        let opts = hub.start("").await;
        let (id, page) = review_page(&hub, &opts).await;
        assert!(
            page.contains("This update adds the passkey &#39;Phone&#39;"),
            "{page}"
        );
        let opts = hub.start(&id).await;
        assert!(opts["challenge"].is_string(), "{opts}");
        let (status, body) = hub.sign(&id).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let mut names: Vec<String> = hub
            .latest_manifest()
            .owner_keys
            .into_iter()
            .map(|k| k.name)
            .collect();
        names.sort();
        assert_eq!(names, vec!["Laptop", "Phone"]);
    }

    #[tokio::test]
    async fn sending_with_nothing_new_says_so_and_sends_nothing() {
        let hub = SendHub::new();
        hub.send_all().await;
        let opts = hub.start("").await;
        assert_eq!(opts["nothing"], true, "{opts}");
        assert_eq!(opts["message"], "Your machines already have everything.");
        let form = format!("csrf={}&next=%2F%3Fsent%3D1&back=%2F", hub.csrf);
        let (status, _) = call(&hub.app, post_form(&hub, "/send/review", form)).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (_, settings) = call(&hub.app, hub.get("/settings?flash=nothing")).await;
        assert!(settings.contains("Your machines already have everything."));
        assert_eq!(hub.writer().releases.numbers(), vec![1]);
    }

    #[tokio::test]
    async fn the_check_page_works_without_the_script() {
        let hub = SendHub::new();
        add_hook(&hub.knowledge, "guard", "echo guard\n");
        let form = format!("csrf={}&next=%2F%3Fsent%3D1&back=%2F", hub.csrf);
        let res = hub
            .app
            .clone()
            .oneshot(post_form(&hub, "/send/review", form))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        let to = res.headers()[header::LOCATION].to_str().unwrap().to_owned();
        assert!(to.starts_with("/send/review/"), "{to}");
        let (_, page) = call(&hub.app, hub.get(&to)).await;
        assert!(
            page.contains("echo guard") && page.contains(r#"href="/">Not now"#),
            "{page}"
        );
    }

    #[tokio::test]
    async fn the_line_beside_the_tap_counts_what_changed() {
        let hub = SendHub::new();
        hub.send_all().await;
        std::fs::write(hub.knowledge.join("AGENTS.md"), "# Rules, changed\n").unwrap();
        for skill in ["a", "b"] {
            let dir = hub.knowledge.join("skills").join(skill);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("SKILL.md"), format!("---\nname: {skill}\n---\n")).unwrap();
        }
        git(&hub.knowledge, &["add", "-A"]);
        git(
            &hub.knowledge,
            &["commit", "-q", "-m", "Edit AGENTS.md in Atlas"],
        );
        let opts = hub.start("").await;
        assert_eq!(opts["summary"], "Update 2 · AGENTS.md and 2 skills changed");
    }

    #[tokio::test]
    async fn links_in_a_hook_folder_are_flagged() {
        use std::os::unix::fs::symlink;
        let hub = SendHub::new();
        std::fs::create_dir_all(hub.knowledge.join("lib")).unwrap();
        std::fs::write(hub.knowledge.join("lib/real.sh"), "echo real\n").unwrap();
        add_hook(&hub.knowledge, "linked", "unused\n");
        let script = hub.knowledge.join("hooks/linked/run.sh");
        std::fs::remove_file(&script).unwrap();
        symlink("../../lib/real.sh", &script).unwrap();
        add_hook(&hub.knowledge, "helped", ". ./helper.sh\n");
        symlink(
            "../../lib/real.sh",
            hub.knowledge.join("hooks/helped/helper.sh"),
        )
        .unwrap();
        std::fs::create_dir_all(hub.knowledge.join("elsewhere/far")).unwrap();
        std::fs::write(hub.knowledge.join("elsewhere/far/hook.toml"), "x").unwrap();
        symlink("../elsewhere/far", hub.knowledge.join("hooks/far")).unwrap();
        git(&hub.knowledge, &["add", "-A"]);
        git(&hub.knowledge, &["commit", "-q", "-m", "links"]);
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        for text in [
            "Its script is a link, so your machines will skip this hook.",
            "Its folder holds a link (helper.sh), so your machines will skip this hook.",
            "Its folder is a link (to ../elsewhere/far), so your machines will skip this hook.",
            "A link to ../../lib/real.sh",
        ] {
            assert!(page.contains(text), "{text}: {page}");
        }
    }

    #[tokio::test]
    async fn a_linked_hooks_folder_is_flagged() {
        use std::os::unix::fs::symlink;
        let hub = SendHub::new();
        hub.send_all().await;
        std::fs::create_dir_all(hub.knowledge.join("stuff")).unwrap();
        std::fs::write(hub.knowledge.join("stuff/x"), "x").unwrap();
        symlink("stuff", hub.knowledge.join("hooks")).unwrap();
        git(&hub.knowledge, &["add", "-A"]);
        git(&hub.knowledge, &["commit", "-q", "-m", "link hooks"]);
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        assert!(
            page.contains("The hooks folder is a link, so your machines will skip every hook."),
            "{page}"
        );
    }

    const SKILL_HOOKS: &str = "---\nname: pdf\ndescription: Reads PDFs\nhooks:\n  PreToolUse:\n    - matcher: Bash\n      hooks:\n        - type: command\n          command: ./check.sh\n---\nRead PDFs.\n";

    #[tokio::test]
    async fn hooks_in_a_skill_or_subagent_get_a_look() {
        let hub = SendHub::new();
        let skill = hub.knowledge.join("skills/pdf");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), "---\nname: pdf\n---\nRead PDFs.\n").unwrap();
        git(&hub.knowledge, &["add", "-A"]);
        git(&hub.knowledge, &["commit", "-q", "-m", "pdf"]);
        hub.send_all().await;

        // A hooks block appears in the skill's frontmatter.
        std::fs::write(skill.join("SKILL.md"), SKILL_HOOKS).unwrap();
        git(&hub.knowledge, &["commit", "-q", "-am", "pdf hooks"]);
        let opts = hub.start("").await;
        let (id, page) = review_page(&hub, &opts).await;
        assert!(page.contains("Hooks in the skill pdf"), "{page}");
        assert!(page.contains("While the skill is in use"), "{page}");
        assert!(page.contains("command: ./check.sh"), "{page}");
        hub.start(&id).await;
        let (status, _) = hub.sign(&id).await;
        assert_eq!(status, StatusCode::OK);

        // Its command changes: a diff.
        std::fs::write(
            skill.join("SKILL.md"),
            SKILL_HOOKS.replace("./check.sh", "./other.sh"),
        )
        .unwrap();
        git(&hub.knowledge, &["commit", "-q", "-am", "pdf other"]);
        let opts = hub.start("").await;
        let (_, page) = review_page(&hub, &opts).await;
        assert!(
            page.contains(r#"<span class="d-del">-          command: ./check.sh</span>"#),
            "{page}"
        );
        assert!(
            page.contains(r#"<span class="d-add">+          command: ./other.sh</span>"#),
            "{page}"
        );

        // Prose changes only: no look.
        let hub2 = SendHub::new();
        let skill = hub2.knowledge.join("skills/pdf");
        std::fs::create_dir_all(&skill).unwrap();
        std::fs::write(skill.join("SKILL.md"), SKILL_HOOKS).unwrap();
        git(&hub2.knowledge, &["add", "-A"]);
        git(&hub2.knowledge, &["commit", "-q", "-m", "pdf"]);
        hub2.send_all().await;
        std::fs::write(
            skill.join("SKILL.md"),
            SKILL_HOOKS.replace("Read PDFs.", "Read them."),
        )
        .unwrap();
        git(&hub2.knowledge, &["commit", "-q", "-am", "prose"]);
        let opts = hub2.start("").await;
        assert!(opts["review"].is_null(), "{opts}");

        // A subagent with hooks.
        std::fs::create_dir_all(hub2.knowledge.join("agents")).unwrap();
        std::fs::write(
            hub2.knowledge.join("agents/reviewer.md"),
            SKILL_HOOKS.replace("name: pdf", "name: reviewer"),
        )
        .unwrap();
        git(&hub2.knowledge, &["add", "-A"]);
        git(&hub2.knowledge, &["commit", "-q", "-m", "reviewer"]);
        let opts = hub2.start("").await;
        let (_, page) = review_page(&hub2, &opts).await;
        assert!(page.contains("Hooks in the subagent reviewer"), "{page}");
        assert!(page.contains("While the subagent is in use"), "{page}");
    }

    #[tokio::test]
    async fn machines_ui_adds_pairs_and_forgets_machines() {
        let hub = SendHub::new();

        // Signed-out visitors see a prompt to sign in rather than a bare page.
        let unsigned = Request::get("/machines")
            .header("tailscale-user-login", LOGIN)
            .body(Body::empty())
            .unwrap();
        let (status, page) = call(&hub.app, unsigned).await;
        assert_eq!(status, StatusCode::OK);
        assert!(page.contains("href=\"/signin?next=/machines\""), "{page}");

        // Add, pair-tailor, and forget a machine via the UI.
        let (status, _) = call(
            &hub.app,
            post_form(
                &hub,
                "/machines/add",
                format!("csrf={}&name=workstation", hub.csrf),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (_, page) = call(&hub.app, hub.get("/machines")).await;
        assert!(page.contains("workstation"), "{page}");

        let (status, page) = call(
            &hub.app,
            post_form(
                &hub,
                "/machines/pair",
                format!("csrf={}&machine=workstation", hub.csrf),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(
            page.contains(&format!(
                "ssh workstation 'curl -fsSL {ORIGIN}/install | sh -s -- "
            )),
            "{page}"
        );

        let (status, _) = call(
            &hub.app,
            post_form(
                &hub,
                "/machines/forget",
                format!("csrf={}&name=workstation", hub.csrf),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let (_, page) = call(&hub.app, hub.get("/machines")).await;
        assert!(!page.contains("workstation"), "{page}");
    }

    async fn mint_cli_token(hub: &SendHub, name: &str) -> String {
        let (status, page) = call(
            &hub.app,
            post_form(
                hub,
                "/settings/tokens",
                format!("csrf={}&name={name}", hub.csrf),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let prefix = format!("atlasd auth {ORIGIN} ");
        let start = page.find(&prefix).expect("raw token shown once") + prefix.len();
        let token = page[start..start + 64].to_owned();
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        token
    }

    fn admin_req(req: axum::http::request::Builder, token: &str) -> axum::http::request::Builder {
        req.header("tailscale-user-login", LOGIN)
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
    }

    #[tokio::test]
    async fn admin_cli_api_manages_machines_behind_cli_tokens() {
        let hub = SendHub::new();
        let cli_token = mint_cli_token(&hub, "laptop").await;

        // Missing token is rejected.
        let no_auth = Request::get("/api/admin/machines")
            .header("tailscale-user-login", LOGIN)
            .body(Body::empty())
            .unwrap();
        assert_eq!(call(&hub.app, no_auth).await.0, StatusCode::UNAUTHORIZED);

        // Add tracked machine, list, and issue a pairing code.
        let add_req = admin_req(Request::post("/api/admin/machines"), &cli_token)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"name":"workstation"}"#))
            .unwrap();
        assert_eq!(call(&hub.app, add_req).await.0, StatusCode::CREATED);

        let (status, body) = call(
            &hub.app,
            admin_req(Request::get("/api/admin/machines"), &cli_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let listed: atlas_core::distribution::MachinesListResponse =
            serde_json::from_str(&body).unwrap();
        assert_eq!(listed.hub, "hub");
        assert!(listed.machines.iter().any(|m| m.name == "workstation"));

        let (status, body) = call(
            &hub.app,
            admin_req(Request::post("/api/admin/pair-code"), &cli_token)
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let issued: atlas_core::distribution::PairCodeResponse =
            serde_json::from_str(&body).unwrap();

        // Pair using the issued code; header dropdown reflects paired state, and ordinary client token is refused on admin API.
        let do_pair = Request::post("/api/pair")
            .header("tailscale-user-login", LOGIN)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(format!(
                r#"{{"code":"{}","hub":"{ORIGIN}","machine":"workstation","user":"me"}}"#,
                issued.code
            )))
            .unwrap();
        let (status, body) = call(&hub.app, do_pair).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let paired: atlas_core::distribution::PairResponse = serde_json::from_str(&body).unwrap();
        let (_, skills_html) = call(&hub.app, hub.get("/skills")).await;
        assert!(
            skills_html.contains("workstation (paired)"),
            "{skills_html}"
        );
        assert!(!skills_html.contains("workstation (not paired)"));
        assert_eq!(
            call(
                &hub.app,
                admin_req(Request::get("/api/admin/machines"), &paired.token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );

        // Removing workstation drops both the tracked entry and its paired client; second time is 404.
        let del = || {
            admin_req(
                Request::delete("/api/admin/machines/workstation"),
                &cli_token,
            )
            .body(Body::empty())
            .unwrap()
        };
        let (status, body) = call(&hub.app, del()).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let removed: atlas_core::distribution::RemoveMachineResponse =
            serde_json::from_str(&body).unwrap();
        assert!(removed.removed_machine);
        assert_eq!(removed.removed_clients, 1);
        assert_eq!(call(&hub.app, del()).await.0, StatusCode::NOT_FOUND);

        // Revoking the CLI token immediately blocks it.
        let tokens = crate::tokens::CliTokens::open(&hub.home.join("state")).list();
        assert_eq!(tokens.len(), 1);
        let (status, _) = call(
            &hub.app,
            post_form(
                &hub,
                &format!("/settings/tokens/{}/remove", tokens[0].id),
                format!("csrf={}", hub.csrf),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        assert_eq!(
            call(
                &hub.app,
                admin_req(Request::get("/api/admin/machines"), &cli_token)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .0,
            StatusCode::UNAUTHORIZED
        );
    }
}
