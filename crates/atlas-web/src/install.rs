//! One-line install of atlasd on another machine:
//! `curl -fsSL <hub>/install | sh -s -- <pairing code>`.
//!
//! The hub serves the script and the client builds it keeps in `<state>/dist/`
//! (`atlasd-linux-x86_64`, `atlasd-linux-aarch64`, `atlasd-macos-x86_64`,
//! `atlasd-macos-aarch64`), behind the same Tailscale login check as every page. The
//! script checks each download against the SHA-256 the hub computed.
//!
//! The script checks that the machine can run atlasd on a timer before it pairs, runs the
//! first pass in the foreground and prints its summary, then starts the timer (decisions:
//! "The install script runs the first pass in the foreground").

use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

use crate::{AppError, AppState};

const ARCHES: [&str; 2] = ["x86_64", "aarch64"];
const SERVICE: &str = include_str!("../../../deploy/atlasd.service");
const TIMER: &str = include_str!("../../../deploy/atlasd.timer");
const PLIST: &str = include_str!("../../../deploy/atlasd.plist");
/// Operating systems atlasd is built for, as they appear in build names (`atlasd-<os>-<arch>`).
const OSES: [&str; 2] = ["linux", "macos"];

/// Said wherever an install can't go ahead because the hub has no builds to hand out.
pub const NO_BUILDS: &str = "This hub has no atlasd builds to hand out yet. Add one with the build step in the hub guide (documents/guide/install-hub.md, \"Give the hub atlasd builds to hand out\").";

fn dist(s: &AppState) -> Result<std::path::PathBuf, AppError> {
    Ok(s.writer
        .as_ref()
        .ok_or(AppError::NotFound)?
        .state_dir
        .join("dist"))
}

/// Whether `<state_dir>/dist` holds at least one atlasd build the script hands out.
#[must_use]
pub fn has_builds(state_dir: &std::path::Path) -> bool {
    let dir = state_dir.join("dist");
    OSES.iter().any(|os| {
        ARCHES
            .iter()
            .any(|arch| dir.join(format!("atlasd-{os}-{arch}")).is_file())
    })
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

fn shell(body: String) -> Response {
    (
        [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
        body,
    )
        .into_response()
}

/// The install script, with this hub's address and each build's checksum filled in.
///
/// With no builds it is a script that says so and exits 1: the documented command runs
/// `curl -f`, which would hide an error status's body behind "error: 400".
pub(crate) async fn script(State(s): State<AppState>) -> Result<Response, AppError> {
    let dir = dist(&s)?;
    let origin = s
        .writer
        .as_ref()
        .map_or_else(String::new, |w| w.origin().to_owned());
    let mut sums = String::new();
    for os in OSES {
        for arch in ARCHES {
            if let Ok(bytes) = tokio::fs::read(dir.join(format!("atlasd-{os}-{arch}"))).await {
                use std::fmt::Write as _;
                let _ = writeln!(sums, "  {os}-{arch}) sum={} ;;", sha256_hex(&bytes));
            }
        }
    }
    if sums.is_empty() {
        return Ok(shell(no_builds_script()));
    }
    Ok(shell(script_text(&origin, &sums)))
}

fn no_builds_script() -> String {
    format!(
        "#!/bin/sh\necho '{}' >&2\necho 'Nothing was installed or paired.' >&2\nexit 1\n",
        NO_BUILDS.replace('\'', r"'\''")
    )
}

/// The install script for a hub at `origin`; `sums` holds one `  <os>-<arch>) sum=<hex> ;;`
/// line per atlasd build. It must run under any POSIX `sh`, macOS's bash 3.2 included.
fn script_text(origin: &str, sums: &str) -> String {
    format!(
        r#"#!/bin/sh
# Installs atlasd for this account and pairs it with the Atlas hub at {origin}.
# Usage: curl -fsSL {origin}/install | sh -s -- <pairing code>
set -eu
# No apostrophes inside ${{...}}: macOS's /bin/sh (bash 3.2) reads one as an open quote.
if [ -z "${{1:-}}" ]; then
  echo "Give the pairing code from the hub's Machines page." >&2
  exit 1
fi
# A code may come in groups (K7QF 9XMR2D); the hub ignores spaces and hyphens.
code="$*"
hub="{origin}"
case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  *) echo "atlasd isn't available for $(uname -s) yet." >&2; exit 1 ;;
esac
case "$(uname -m)" in
  x86_64|amd64) arch=x86_64 ;;
  aarch64|arm64) arch=aarch64 ;;
  *) echo "atlasd isn't available for $(uname -m) yet." >&2; exit 1 ;;
esac
case "$os-$arch" in
{sums}  *) echo "This hub has no atlasd build for $os on $arch." >&2; exit 1 ;;
esac

# atlasd runs from a user service. Check for one before pairing, so a machine that
# can't run it is never left paired and silent.
uid="$(id -u)"
if [ "$os" = macos ]; then
  if ! launchctl print "user/$uid" >/dev/null 2>&1 && ! launchctl print "gui/$uid" >/dev/null 2>&1; then
    echo "launchd has no session for this account, so atlasd couldn't run on its own." >&2
    echo "Nothing was installed or paired." >&2
    exit 1
  fi
elif ! command -v systemctl >/dev/null 2>&1 || ! systemctl --user show-environment >/dev/null 2>&1; then
  echo "This machine has no systemd user services (WSL and containers often don't), so atlasd couldn't run on its own." >&2
  echo "Nothing was installed or paired." >&2
  exit 1
fi

echo "Downloading atlasd for $os on $arch…"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
# Not curl -f, which hides why: show the hub's own message when it refuses.
status="$(curl -sSL -w '%{{http_code}}' -o "$tmp" "$hub/install/atlasd-$os-$arch")" || {{
  echo "Couldn't download atlasd from $hub. Nothing was installed or paired." >&2
  exit 1
}}
if [ "$status" != 200 ]; then
  echo "The hub refused the download (HTTP $status):" >&2
  head -c 2000 "$tmp" >&2
  echo >&2
  echo "Nothing was installed or paired." >&2
  exit 1
fi
if command -v sha256sum >/dev/null 2>&1; then
  echo "$sum  $tmp" | sha256sum -c --quiet -
else
  echo "$sum  $tmp" | shasum -a 256 -c --quiet -
fi
mkdir -p "$HOME/.local/bin"
install -m 755 "$tmp" "$HOME/.local/bin/atlasd"

"$HOME/.local/bin/atlasd" pair "$hub" "$code"

# The first pass, here, so its summary shows; then the timer or agent takes over.
echo
if ! "$HOME/.local/bin/atlasd" run --summary; then
  echo "The first run had problems (above). atlasd tries again every 10 minutes." >&2
fi
echo

if [ "$os" = macos ]; then
  # A launchd agent for this login runs atlasd every 10 minutes (atlasd.timer on Linux).
  agents="$HOME/Library/LaunchAgents"
  mkdir -p "$agents" "$HOME/Library/Logs"
  cat <<'PLIST' | sed "s|__HOME__|$HOME|g" > "$agents/atlasd.plist"
{PLIST}PLIST
  launchctl bootout "gui/$uid/atlasd" 2>/dev/null || launchctl bootout "user/$uid/atlasd" 2>/dev/null || true
  # gui/ is the logged-in desktop session; over SSH with nobody logged in, only user/ exists.
  launchctl bootstrap "gui/$uid" "$agents/atlasd.plist" 2>/dev/null ||
    launchctl bootstrap "user/$uid" "$agents/atlasd.plist"
  echo "Done. atlasd checks for updates every 10 minutes (log: ~/Library/Logs/atlasd.log)."
  exit 0
fi

units="${{XDG_CONFIG_HOME:-$HOME/.config}}/systemd/user"
mkdir -p "$units"
cat > "$units/atlasd.service" <<'UNIT'
{SERVICE}UNIT
cat > "$units/atlasd.timer" <<'UNIT'
{TIMER}UNIT
systemctl --user daemon-reload
systemctl --user enable --now atlasd.timer
if ! loginctl enable-linger "${{USER:-$(id -un)}}" >/dev/null 2>&1; then
  echo "Couldn't turn on lingering (loginctl enable-linger), so atlasd only runs while you're logged in."
fi
echo "Done. atlasd checks for updates every 10 minutes (log: journalctl --user -u atlasd)."
"#
    )
}

/// A client build, by operating system and architecture.
pub(crate) async fn binary(
    State(s): State<AppState>,
    Path(name): Path<String>,
) -> Result<Response, AppError> {
    let known = OSES.iter().any(|os| {
        ARCHES
            .iter()
            .any(|arch| name == format!("atlasd-{os}-{arch}"))
    });
    if !known {
        return Err(AppError::NotFound);
    }
    let bytes = tokio::fs::read(dist(&s)?.join(&name))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn script() -> String {
        script_text(
            "https://hub.example.ts.net:7443",
            "  linux-x86_64) sum=00 ;;\n  macos-aarch64) sum=00 ;;\n",
        )
    }

    /// macOS's /bin/sh (bash 3.2) treats an apostrophe inside `${...}` as an open quote and
    /// fails to parse the whole script, so pairing a Mac said "unexpected EOF" instead of
    /// that macOS isn't supported.
    #[test]
    fn no_apostrophe_inside_a_parameter_expansion() {
        let text = script();
        for (n, line) in text.lines().enumerate() {
            let mut rest = line;
            while let Some(start) = rest.find("${") {
                let inner = &rest[start + 2..];
                let end = inner.find('}').unwrap_or(inner.len());
                assert!(!inner[..end].contains('\''), "line {}: {line}", n + 1);
                rest = &inner[end..];
            }
        }
    }

    #[test]
    fn parses_and_asks_for_a_code() {
        let text = script();
        let parse = Command::new("sh")
            .args(["-n", "-c", &text])
            .output()
            .unwrap();
        assert!(parse.status.success(), "{parse:?}");
        let run = Command::new("sh")
            .args(["-c", &text, "sh"])
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(1));
        assert!(
            String::from_utf8_lossy(&run.stderr).contains("Give the pairing code"),
            "{run:?}"
        );
    }

    fn at(text: &str, needle: &str) -> usize {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle:?} not in the script"))
    }

    /// The script's commands, without its comments or the unit files it writes.
    fn commands(text: &str) -> String {
        text.lines()
            .map(str::trim_start)
            .filter(|l| !l.starts_with('#') && !l.starts_with("<!--"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The owner's old folders aren't everyone's: the served script adopts nothing.
    #[test]
    fn adopts_no_folders() {
        let run = commands(&script());
        assert!(!run.contains("--adopt"), "{run}");
        assert!(!run.contains("Knowledge"), "{run}");
    }

    /// A machine without user services stops before pairing, so it never sits paired
    /// and silent; the first pass runs in the foreground before the timer starts.
    #[test]
    fn checks_services_then_pairs_then_runs_then_starts_the_timer() {
        let text = script();
        let check = at(&text, "systemctl --user show-environment");
        let launchd = at(&text, "launchctl print \"user/$uid\"");
        let download = at(&text, "curl -sSL");
        let pair = at(&text, "atlasd\" pair \"$hub\" \"$code\"");
        let run = at(&text, "atlasd\" run --summary");
        let timer = at(&text, "systemctl --user enable --now atlasd.timer");
        let agent = at(&text, "launchctl bootstrap");
        assert!(check < download && launchd < download && download < pair);
        assert!(pair < run && run < timer && run < agent);
        assert!(!text.contains("start --no-block"), "no second first run");
    }

    #[test]
    fn says_what_failed_lingering_means() {
        let text = script();
        let linger = at(&text, "if ! loginctl enable-linger");
        assert!(text[linger..].contains("atlasd only runs while you're logged in"));
    }

    /// The binary download shows the hub's message instead of curl -f's bare status,
    /// and the checksum is still checked.
    #[test]
    fn a_refused_download_shows_the_hubs_message() {
        let text = script();
        assert!(
            !commands(&text).contains("curl -f"),
            "only the documented command uses -f"
        );
        let status = at(&text, "-w '%{http_code}'");
        let shown = at(&text, "head -c 2000 \"$tmp\" >&2");
        let sum = at(&text, "sha256sum -c");
        assert!(status < shown && shown < sum);
    }

    #[test]
    fn with_no_builds_the_script_says_so() {
        let text = no_builds_script();
        let run = Command::new("sh")
            .args(["-c", &text, "sh", "code"])
            .output()
            .unwrap();
        assert_eq!(run.status.code(), Some(1));
        let err = String::from_utf8_lossy(&run.stderr);
        assert!(err.contains("no atlasd builds"), "{err}");
        assert!(
            err.contains("Give the hub atlasd builds to hand out"),
            "{err}"
        );
        assert!(err.contains("Nothing was installed or paired."), "{err}");
    }
}
