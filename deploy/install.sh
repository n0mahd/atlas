#!/usr/bin/env bash
# Install the Atlas hub for this user and open the page that creates its first passkey.
#
#   deploy/install.sh https://<this-machine>.<tailnet>.ts.net:<port>
#
# Needs: Rust (cargo), git, Tailscale with HTTPS certificates enabled for the tailnet.
# Writes ~/.config/atlas/hub.toml only if it doesn't exist yet.
set -euo pipefail

url="${1:-}"
port="${url##*:}"
if [[ ! "$url" =~ ^https://[^/]+:[0-9]+$ ]]; then
  echo "usage: $0 https://<this-machine>.<tailnet>.ts.net:<port>" >&2
  exit 2
fi
repo="$(cd "$(dirname "$0")/.." && pwd)"

echo "Building…"
cargo build --release --manifest-path "$repo/Cargo.toml" -p atlas-hub
install -Dm755 "$repo/target/release/atlas-hub" "$HOME/.local/bin/atlas-hub"

config="$HOME/.config/atlas/hub.toml"
if [[ ! -e "$config" ]]; then
  login="$(tailscale status --json | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d["User"][str(d["Self"]["UserID"])]["LoginName"])')"
  mkdir -p "$(dirname "$config")"
  cat > "$config" <<TOML
allowed_logins = ["$login"]
knowledge = "~/Knowledge"
public_skills = "~/Skills"
projects_dir = "~/Projects"
public_url = "$url"
TOML
  echo "Wrote $config for $login. Edit it if your folders differ."
fi

mkdir -p "$HOME/.config/systemd/user"
ln -sf "$repo/deploy/atlas-hub.service" "$HOME/.config/systemd/user/atlas-hub.service"
systemctl --user daemon-reload
systemctl --user enable --now atlas-hub
systemctl --user restart atlas-hub
loginctl enable-linger "$USER" 2>/dev/null || true

socket="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/atlas/hub.sock"
serve=(tailscale serve --bg --https="$port" "unix:$socket")
if ! "${serve[@]}" >/dev/null 2>&1; then
  echo "Putting the hub on the tailnet needs Tailscale operator rights:"
  sudo "${serve[@]}" >/dev/null
fi

echo
setup="$("$HOME/.local/bin/atlas-hub" setup)"
printf '%s\n' "$setup"
link="$(grep -o 'https://[^ ]*/setup/[0-9a-f]*' <<<"$setup" | head -1)"
if [[ -n "$link" ]] && command -v xdg-open >/dev/null && [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
  xdg-open "$link" >/dev/null 2>&1 &
  echo "Opened the link in your browser."
fi
