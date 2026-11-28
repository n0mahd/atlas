#!/usr/bin/env bash
# Install the Atlas hub for this user and open the page that creates its first passkey.
#
#   deploy/install.sh [--download | --build] https://<this-machine>.<tailnet>.ts.net:<port>
#
# Linux only. Downloads prebuilt binaries from the latest GitHub release by default (when
# one is available for this CPU), falling back to building from source with cargo. Pass
# --download to require a release (no Rust toolchain needed) or --build to compile from
# this checkout. Needs git, systemd user services, and Tailscale signed in with HTTPS
# certificates turned on. Checks all of that before downloading or building, lists what
# it will create, and never changes an existing hub.toml or knowledge repo.
set -euo pipefail

say() { printf '%s\n' "$*"; }
die() {
  printf '%s\n' "$*" >&2
  exit 1
}

repo="$(cd "$(dirname "$0")/.." && pwd)"
# rustup picks the toolchain pinned in rust-toolchain.toml from the working folder.
cd "$repo"
target="${CARGO_TARGET_DIR:-$repo/target}"
user="${USER:-$(id -un)}"
release_url="${ATLAS_RELEASE_URL:-https://github.com/n0mahd/atlas/releases/latest/download}"

tmp=""
cleanup() {
  if [[ -n "$tmp" ]]; then
    rm -rf "$tmp"
  fi
}
trap cleanup EXIT

# --- Checks, before anything is downloaded, built or written -------------------------

[[ "$(uname -s)" == Linux ]] ||
  die "The Atlas hub runs on Linux only, so it can't be installed on this $(uname -s) machine. (atlasd, the client, also runs on macOS.)"
command -v git >/dev/null ||
  die "git isn't installed. Install it with your package manager and run this again."
command -v tailscale >/dev/null ||
  die "tailscale isn't installed. Install it from https://tailscale.com/download and sign in with 'tailscale up'."
self_ip="$(tailscale ip -4 2>/dev/null | head -1)" || true
[[ -n "$self_ip" ]] ||
  die "Tailscale isn't signed in on this machine. Run 'tailscale up' (or 'sudo tailscale up') and run this again."
{ command -v systemctl >/dev/null && systemctl --user show-environment >/dev/null 2>&1; } ||
  die "systemd user services aren't available here ('systemctl --user' fails). The hub needs them; containers and WSL often lack a user session."

whois="$(tailscale whois "$self_ip" 2>/dev/null)" || true
login="$(awk '/^User:/ { u = 1; next } u && $1 == "Name:" { print $2; exit }' <<<"$whois")"
machine="$(awk '/^Machine:/ { m = 1; next } m && $1 == "Name:" { print $2; exit }' <<<"$whois")"
machine="${machine%.}"

usage() {
  say "usage: $0 [--download|--build] https://<this-machine>.<tailnet>.ts.net:<port>" >&2
  [[ -n "$machine" ]] && say "For this machine that would be, for example: $0 https://$machine:7443" >&2
  exit 2
}

mode="auto"
url=""
while (($#)); do
  case "$1" in
    --download)
      [[ "$mode" == build ]] && die "Pass either --download or --build, not both."
      mode="download"
      ;;
    --build)
      [[ "$mode" == download ]] && die "Pass either --download or --build, not both."
      mode="build"
      ;;
    --)
      shift
      if (($# == 1)) && [[ -z "$url" ]]; then
        url="$1"
      else
        usage
      fi
      break
      ;;
    -*)
      usage
      ;;
    *)
      [[ -z "$url" ]] || usage
      url="$1"
      ;;
  esac
  shift
done

if [[ ! "$url" =~ ^https://[^/]+:[0-9]+$ ]]; then
  usage
fi
port="${url##*:}"
host="${url#https://}"
host="${host%:*}"
if [[ -n "$machine" && "$host" != "$machine" ]]; then
  say "Note: this machine's Tailscale name is $machine, not $host. Passkeys are tied to the address, so check it before you go on."
fi

config="$HOME/.config/atlas/hub.toml"
unit="$HOME/.config/systemd/user/atlas-hub.service"
bin="$HOME/.local/bin/atlas-hub"
state="${XDG_STATE_HOME:-$HOME/.local/state}/atlas-hub"

# A plain `key = "value"` from the existing hub.toml, with ~/ expanded.
toml_value() {
  local v
  v="$(sed -nE "s/^[[:space:]]*$1[[:space:]]*=[[:space:]]*\"([^\"]*)\".*/\1/p" "$config" | head -1)"
  [[ "$v" == \~/* ]] && v="$HOME/${v#\~/}"
  printf '%s' "$v"
}

write_config=0
starter=0
if [[ -e "$config" ]]; then
  knowledge="$(toml_value knowledge)"
  custom_state="$(toml_value state_dir)"
  [[ -n "$custom_state" ]] && state="$custom_state"
else
  write_config=1
  knowledge="$HOME/Knowledge"
  [[ -e "$knowledge" ]] || starter=1
  if [[ -z "$login" || "$login" != *@* ]]; then
    die "Couldn't tell which Tailscale login this machine uses (a tagged device has none). Copy deploy/hub.example.toml to $config, put your login in allowed_logins, and run this again."
  fi
fi
if ((starter)); then
  # From / so this clone's own git config doesn't count.
  { git -C / config user.name >/dev/null && git -C / config user.email >/dev/null; } ||
    die "git has no name and email yet, and the hub commits every save. Run: git config --global user.name 'Your Name'; git config --global user.email you@example.com"
fi

arch="$(uname -m)"
dist="$state/dist/atlasd-linux-$arch"
[[ "$arch" == x86_64 || "$arch" == aarch64 ]] || dist=""

have_sha256() {
  command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1
}

fetch_sums() {
  [[ -n "$tmp" ]] || tmp="$(mktemp -d)"
  curl -fsSL --connect-timeout 10 --max-time 30 -o "$tmp/SHA256SUMS" "$release_url/SHA256SUMS" 2>/dev/null
}

expected_sum() {
  awk -v f="$1" '$2 == f && $1 ~ /^[0-9a-fA-F]{64}$/ { print $1; exit }' "$tmp/SHA256SUMS"
}

verify_sha256() {
  local file="$1" expected="$2"
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s  %s\n' "$expected" "$file" | sha256sum -c --quiet -
  else
    printf '%s  %s\n' "$expected" "$file" | shasum -a 256 -c --quiet -
  fi
}

use_download=0
hub_asset="atlas-hub-linux-$arch"
client_assets=()

if [[ "$mode" == build ]]; then
  [[ -f "$repo/Cargo.toml" ]] ||
    die "This folder isn't a source checkout (no Cargo.toml), so --build can't run here."
  command -v cargo >/dev/null ||
    die "cargo isn't installed. Install the Rust toolchain from https://rustup.rs and run this again."
elif [[ "$mode" == download ]]; then
  [[ "$arch" == x86_64 || "$arch" == aarch64 ]] ||
    die "Prebuilt hub builds are available for x86_64 and aarch64 only, not $arch. Run with --build on a checkout instead."
  command -v curl >/dev/null ||
    die "curl isn't installed. Install curl or run with --build to compile from source."
  have_sha256 ||
    die "Neither sha256sum nor shasum is installed, so downloaded builds can't be verified."
  fetch_sums ||
    die "Couldn't download SHA256SUMS from $release_url. Nothing was installed."
  [[ -n "$(expected_sum "$hub_asset")" ]] ||
    die "The release has no $hub_asset entry in SHA256SUMS. Nothing was installed."
  use_download=1
else
  fallback_reason=""
  if [[ "$arch" != x86_64 && "$arch" != aarch64 ]]; then
    fallback_reason="Prebuilt hub builds are available for x86_64 and aarch64 only, not $arch; building from source instead."
  elif ! command -v curl >/dev/null; then
    fallback_reason="curl isn't installed, so a release can't be downloaded; building from source instead."
  elif ! have_sha256; then
    fallback_reason="sha256sum isn't installed, so a release can't be verified; building from source instead."
  elif ! fetch_sums; then
    fallback_reason="No published release is available at $release_url; building from source instead."
  elif [[ -z "$(expected_sum "$hub_asset")" ]]; then
    fallback_reason="The latest release has no $hub_asset build; building from source instead."
  else
    use_download=1
  fi
  if ((!use_download)); then
    [[ -f "$repo/Cargo.toml" ]] ||
      die "No published release is available and this folder isn't a source checkout. Nothing was installed."
    command -v cargo >/dev/null ||
      die "No published release is available and cargo isn't installed. Install the Rust toolchain from https://rustup.rs or run again once a release is published."
    say "$fallback_reason"
  fi
fi

if ((use_download)); then
  for name in atlasd-linux-x86_64 atlasd-linux-aarch64 atlasd-macos-x86_64 atlasd-macos-aarch64; do
    [[ -n "$(expected_sum "$name")" ]] && client_assets+=("$name")
  done
fi

# --- Say what will change ------------------------------------------------------------

say "This will:"
if ((use_download)); then
  say "  - download the hub ($arch) from the latest GitHub release, verify its SHA-256, and install it as $bin"
else
  say "  - build the hub and install it as $bin"
fi
if ((write_config)); then
  say "  - write $config, letting in the Tailscale login $login"
else
  say "  - keep your existing $config as it is"
fi
((starter)) && say "  - create a starter knowledge repo in $knowledge (git, branch main)"
say "  - install the user service $unit, start it, and keep it running after you log out"
if ((use_download)); then
  if ((${#client_assets[@]})); then
    say "  - download atlasd builds (${client_assets[*]}) into $state/dist, verified by SHA-256, for the hub to hand out"
  else
    say "  - skip atlasd: the release lists no atlasd builds in SHA256SUMS"
  fi
elif [[ -n "$dist" ]]; then
  say "  - build atlasd for $arch machines and put it in $dist, for the hub to hand out"
else
  say "  - skip building atlasd: the hub hands out builds for x86_64 and aarch64 only, not $arch"
fi
say "  - serve the hub on your Tailscale network at $url (tailscale serve, port $port)"
if [[ -t 0 ]]; then
  read -r -p "Continue? [Y/n] " answer
  [[ "$answer" =~ ^[Nn] ]] && exit 1
fi

# --- Download or build ---------------------------------------------------------------

hub_bin=""
built_atlasd=""
if ((use_download)); then
  download_and_verify() {
    local name="$1" sum
    sum="$(expected_sum "$name")"
    [[ -n "$sum" ]] || die "SHA256SUMS has no entry for $name. Nothing was installed."
    say "Downloading $name…"
    curl -fsSL --connect-timeout 10 --max-time 120 -o "$tmp/$name" "$release_url/$name" ||
      die "Couldn't download $name from $release_url. Nothing was installed."
    verify_sha256 "$tmp/$name" "$sum" >/dev/null 2>&1 ||
      die "Checksum mismatch for $name. Nothing was installed."
  }

  download_and_verify "$hub_asset"
  hub_bin="$tmp/$hub_asset"
  if ((${#client_assets[@]})); then
    for name in "${client_assets[@]}"; do
      download_and_verify "$name"
    done
  fi
else
  say "Building the hub (this takes a few minutes the first time)…"
  cargo build --release -p atlas-hub
  hub_bin="$target/release/atlas-hub"

  # Prefers a static musl build, which runs on any Linux machine of that CPU.
  if [[ -n "$dist" ]]; then
    musl="$arch-unknown-linux-musl"
    targets="$(rustup target list --installed 2>/dev/null)" || true
    if grep -qx "$musl" <<<"$targets"; then
      say "Building atlasd (static, $musl)…"
      if command -v cargo-zigbuild >/dev/null && command -v zig >/dev/null; then
        cargo zigbuild --release -p atlas-client --target "$musl" && built_atlasd="$target/$musl/release/atlasd"
      elif cargo build --release -p atlas-client --target "$musl"; then
        built_atlasd="$target/$musl/release/atlasd"
      fi
      [[ -n "$built_atlasd" ]] || say "The static build didn't link, so atlasd is built for this system instead."
    fi
    if [[ -z "$built_atlasd" ]]; then
      say "Building atlasd for this system. It will suit only $arch machines with a similar or newer Linux; see the guide for builds that run anywhere."
      cargo build --release -p atlas-client
      built_atlasd="$target/release/atlasd"
    fi
  fi
fi

# --- Install -------------------------------------------------------------------------

install -Dm755 "$hub_bin" "$bin"
if ((use_download)); then
  if ((${#client_assets[@]})); then
    # The service wants its state folder closed to others (StateDirectoryMode=0700).
    install -d -m700 "$state" "$state/dist"
    for name in "${client_assets[@]}"; do
      install -m755 "$tmp/$name" "$state/dist/$name"
    done
  fi
elif [[ -n "$built_atlasd" ]]; then
  # The service wants its state folder closed to others (StateDirectoryMode=0700).
  install -d -m700 "$state" "$state/dist"
  install -m755 "$built_atlasd" "$dist"
fi

if ((starter)); then
  mkdir -p "$knowledge"/{skills,memory,hooks,agents}
  touch "$knowledge"/{skills,memory,hooks,agents}/.gitkeep
  cat >"$knowledge/AGENTS.md" <<'MD'
# Agent instructions

Atlas gives these instructions to every agent on every machine it manages. Edit them in the hub.
MD
  git -C "$knowledge" init -q -b main
  git -C "$knowledge" add -A
  git -C "$knowledge" commit -q -m "Start the knowledge repo"
  say "Created $knowledge."
fi

if ((write_config)); then
  mkdir -p "$(dirname "$config")"
  {
    say "allowed_logins = [\"$login\"]"
    say "knowledge = \"~/Knowledge\""
    [[ -d "$HOME/Skills" ]] && say "public_skills = \"~/Skills\""
    [[ -d "$HOME/Projects" ]] && say "projects_dir = \"~/Projects\""
    say "public_url = \"$url\""
  } >"$config"
  say "Wrote $config. See deploy/hub.example.toml for the other settings."
elif [[ -n "$knowledge" && ! -d "$knowledge" ]]; then
  say "Warning: $config names $knowledge as your knowledge repo, but there is no such folder. The hub shows nothing until it exists."
fi

# A copy, not a link into this clone, so moving or deleting the clone leaves the hub alone.
rm -f "$unit"
install -Dm644 "$repo/deploy/atlas-hub.service" "$unit"
systemctl --user daemon-reload
systemctl --user enable atlas-hub
systemctl --user restart atlas-hub
if ! loginctl enable-linger "$user" 2>/dev/null; then
  say "Warning: couldn't keep your services running after you log out, so the hub stops when you do. To fix it, run: sudo loginctl enable-linger $user"
fi

socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atlas/hub.sock"
serve=(tailscale serve --bg --https="$port" "unix:$socket")
say "Putting the hub on your Tailscale network. If Tailscale shows a link to turn on HTTPS or Serve, open it; this goes on once you have."
if ! "${serve[@]}"; then
  say "Trying again with sudo, since Tailscale may want operator rights:"
  sudo "${serve[@]}" ||
    die "tailscale serve failed (its message is above). Fix that, then run: ${serve[*]}"
fi

# Restart=on-failure keeps a crashing hub "activating", so a short wait tells them apart.
sleep 2
if ! systemctl --user is-active --quiet atlas-hub; then
  say "The hub service isn't running. Its last log lines:" >&2
  journalctl --user -u atlas-hub -n 20 --no-pager >&2 || true
  exit 1
fi

if ((starter)); then
  say
  say "One thing left for the knowledge repo: machines fetch updates from its git remote, so"
  say "give it one every machine can reach (a private GitHub repo, say) and push it:"
  say "  git -C $knowledge remote add origin <url> && git -C $knowledge push -u origin main"
elif [[ -d "$knowledge/.git" ]] && ! git -C "$knowledge" rev-parse --abbrev-ref '@{upstream}' >/dev/null 2>&1; then
  say
  say "Note: $knowledge has no upstream branch yet. Machines fetch from it, so add a remote and run: git -C $knowledge push -u origin main"
fi

say
setup="$(STATE_DIRECTORY="$state" "$bin" setup)"
printf '%s\n' "$setup"
link="$(grep -o 'https://[^ ]*/setup/[0-9a-f]*' <<<"$setup" | head -1)" || true
if [[ -n "$link" ]] && command -v xdg-open >/dev/null && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  xdg-open "$link" >/dev/null 2>&1 &
  say "Opened the link in your browser."
fi
say "Next: in the hub, Machines → Pair, and run the command on each machine, this one too if you run agents here."
