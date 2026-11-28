//! One-line install of atlasd on another machine:
//! `curl -fsSL <hub>/install | sh -s -- <pairing code>`.
//!
//! The hub serves the script and the client builds it keeps in `<state>/dist/`
//! (`atlasd-linux-x86_64`, `atlasd-linux-aarch64`), behind the same Tailscale login check
//! as every page. The script checks each download against the SHA-256 the hub computed.

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::{AppError, AppState};

const ARCHES: [&str; 2] = ["x86_64", "aarch64"];
const SERVICE: &str = include_str!("../../../deploy/atlasd.service");
const TIMER: &str = include_str!("../../../deploy/atlasd.timer");

fn dist(s: &AppState) -> Result<std::path::PathBuf, AppError> {
    Ok(s.writer
        .as_ref()
        .ok_or(AppError::NotFound)?
        .state_dir
        .join("dist"))
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut out, b| {
            use std::fmt::Write as _;
            let _ = write!(out, "{b:02x}");
            out
        })
}

/// The install script, with this hub's address and each build's checksum filled in.
pub(crate) async fn script(State(s): State<AppState>) -> Result<Response, AppError> {
    let dir = dist(&s)?;
    let origin = s
        .writer
        .as_ref()
        .map_or_else(String::new, |w| w.origin().to_owned());
    let mut sums = String::new();
    for arch in ARCHES {
        if let Ok(bytes) = tokio::fs::read(dir.join(format!("atlasd-linux-{arch}"))).await {
            use std::fmt::Write as _;
            let _ = writeln!(sums, "  {arch}) sum={} ;;", sha256_hex(&bytes));
        }
    }
    if sums.is_empty() {
        return Err(AppError::Bad(
            "This hub has no atlasd builds to hand out yet.".into(),
        ));
    }
    let body = format!(
        r#"#!/bin/sh
# Installs atlasd for this account and pairs it with the Atlas hub at {origin}.
# Usage: curl -fsSL {origin}/install | sh -s -- <pairing code>
set -eu
code="${{1:?Give the pairing code from the hub's Machines page}}"
hub="{origin}"
case "$(uname -s)" in
  Linux) ;;
  *) echo "atlasd isn't available for $(uname -s) yet." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) echo "atlasd isn't available for $(uname -m) yet." >&2; exit 1 ;;
esac
case "$arch" in
{sums}  *) echo "This hub has no atlasd build for $arch." >&2; exit 1 ;;
esac

echo "Downloading atlasd for $arch…"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL "$hub/install/atlasd-linux-$arch" -o "$tmp"
echo "$sum  $tmp" | sha256sum -c --quiet -
mkdir -p "$HOME/.local/bin"
install -m 755 "$tmp" "$HOME/.local/bin/atlasd"

# Folders an older setup linked from, so atlasd may take those links over.
set --
for dir in "$HOME/Knowledge" "$HOME/Skills"; do
  [ -d "$dir" ] && set -- "$@" --adopt "$dir"
done
"$HOME/.local/bin/atlasd" pair "$hub" "$code" "$@"

units="${{XDG_CONFIG_HOME:-$HOME/.config}}/systemd/user"
mkdir -p "$units"
cat > "$units/atlasd.service" <<'UNIT'
{SERVICE}UNIT
cat > "$units/atlasd.timer" <<'UNIT'
{TIMER}UNIT
systemctl --user daemon-reload
systemctl --user enable --now atlasd.timer
systemctl --user start --no-block atlasd.service 2>/dev/null || true
loginctl enable-linger "$USER" 2>/dev/null || true

echo
echo "Done. atlasd checks for changes every 10 minutes."
echo "To see what it would change now: atlasd run --dry-run"
"#
    );
    Ok((
        [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
        body,
    )
        .into_response())
}

/// A client build, by architecture.
pub(crate) async fn binary(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let arch = name
        .strip_prefix("atlasd-linux-")
        .filter(|a| ARCHES.contains(a))
        .ok_or(AppError::NotFound)?;
    let bytes = tokio::fs::read(dist(&s)?.join(format!("atlasd-linux-{arch}")))
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        bytes,
    )
        .into_response())
}
