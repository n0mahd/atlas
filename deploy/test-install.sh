#!/usr/bin/env bash
# shellcheck disable=SC2016 # the stubs are single-quoted on purpose: they expand when run
# Runs deploy/install.sh in throwaway homes, with stand-ins for tailscale, systemctl,
# loginctl, journalctl, sudo, rustup and cargo first (and only) on PATH, so nothing
# touches this machine's services, Tailscale or Rust builds. Takes about half a minute.
#
#   deploy/test-install.sh
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
root="$(mktemp -d)"
trap 'rm -rf "$root"' EXIT
real_git="$(command -v git)"
failures=0

# Host tools the script may use; everything else is a stub or missing.
host_tools=(bash sh env dirname id cat sed awk grep head tail mkdir touch rm install
  cp chmod mktemp sleep git ls sha256sum tar gzip)

stub() { # name body
  rm -f "$bin/$1"
  printf '#!/usr/bin/env bash\n%s\n' "$2" >"$bin/$1"
  chmod +x "$bin/$1"
}

# Build a fake GitHub release directory with binaries, deploy archive and SHA256SUMS.
make_release() {
  local dir="$1" arch
  arch="$(uname -m)"
  mkdir -p "$dir"
  printf '#!/usr/bin/env bash\necho "Open this link to add a passkey (works once, for 15 minutes):"\necho\necho "  https://hub.example.ts.net:7443/setup/0123abcd"\n' >"$dir/atlas-hub-linux-$arch"
  chmod +x "$dir/atlas-hub-linux-$arch"
  local f
  for f in atlasd-linux-x86_64 atlasd-linux-aarch64 atlasd-macos-x86_64 atlasd-macos-aarch64; do
    printf 'prebuilt %s\n' "$f" >"$dir/$f"
    chmod +x "$dir/$f"
  done
  tar -C "$repo" -czf "$dir/atlas-deploy.tar.gz" deploy/install.sh deploy/atlas-hub.service deploy/hub.example.toml
  (cd "$dir" && LC_ALL=C sha256sum atlas-deploy.tar.gz "atlas-hub-linux-$arch" atlasd-linux-x86_64 atlasd-linux-aarch64 atlasd-macos-x86_64 atlasd-macos-aarch64 >SHA256SUMS)
}

# A fresh world: $home, $bin, $log. Pass tool names to leave out.
world() {
  local name="$1"
  shift
  w="$root/$name"
  home="$w/home" bin="$w/bin" log="$w/calls.log"
  mkdir -p "$home" "$bin" "$w/state" "$w/run" "$w/target"
  : >"$log"
  printf '[user]\n\tname = Test\n\temail = test@example.com\n' >"$home/.gitconfig"
  local t
  for t in "${host_tools[@]}"; do ln -s "$(command -v "$t")" "$bin/$t"; done
  stub uname '[[ "$1" == -s && -n "${STUB_OS:-}" ]] && { echo "$STUB_OS"; exit; }; exec '"$(command -v uname)"' "$@"'
  # Stand-in curl: serves files from STUB_RELEASE_DIR by basename, or 404s when unset.
  stub curl 'echo "curl $*" >>"$STUB_LOG"
out=""; url=""
while (($#)); do
  case "$1" in
    -o) out="$2"; shift ;;
    --connect-timeout|--max-time) shift ;;
    -*) ;;
    *) url="$1" ;;
  esac
  shift
done
[[ -n "${STUB_RELEASE_DIR:-}" ]] || { echo "curl: (22) The requested URL returned error: 404" >&2; exit 22; }
file="$STUB_RELEASE_DIR/${url##*/}"
[[ -f "$file" ]] || { echo "curl: (22) The requested URL returned error: 404" >&2; exit 22; }
if [[ -n "$out" ]]; then cp "$file" "$out"; else cat "$file"; fi'
  stub tailscale 'echo "tailscale $*" >>"$STUB_LOG"
case "$1" in
  ip) [[ -n "${STUB_LOGGED_OUT:-}" ]] && { echo "no current Tailscale IPs; state: NeedsLogin" >&2; exit 1; }; echo 100.64.0.1 ;;
  whois) printf "Machine:\n  Name:          hub.example.ts.net.\n  ID:            1\nUser:\n  Name:     you@example\n  ID:       2\n" ;;
  serve) [[ -n "${STUB_SERVE_FAIL:-}" ]] && { echo "Serve is not enabled on your tailnet. To enable, visit: https://login.tailscale.com/f/serve?node=x" >&2; exit 1; }
         echo "Available within your tailnet: https://hub.example.ts.net:7443/" ;;
esac'
  stub systemctl 'echo "systemctl $*" >>"$STUB_LOG"
case "$*" in
  "--user show-environment") [[ -z "${STUB_NO_USER_BUS:-}" ]] || { echo "Failed to connect to bus" >&2; exit 1; } ;;
  "--user is-active"*) [[ -z "${STUB_INACTIVE:-}" ]] ;;
esac'
  stub journalctl 'echo "atlas-hub: allowed_logins is empty, so nobody could get in"'
  stub loginctl 'echo "loginctl $*" >>"$STUB_LOG"; [[ -z "${STUB_NO_LINGER:-}" ]]'
  stub sudo 'echo "sudo $*" >>"$STUB_LOG"; exec "$@"'
  stub rustup '[[ -n "${STUB_MUSL:-}" ]] && echo "$(uname -m)-unknown-linux-musl"; echo "$(uname -m)-unknown-linux-gnu"'
  # Writes a stand-in binary where cargo would; the hub's `setup` prints a link.
  stub cargo 'echo "cargo $*" >>"$STUB_LOG"
t=""; p=""; while (($#)); do case "$1" in --target) t="$2/"; shift ;; -p) p="$2" ;; esac; shift; done
[[ -n "$t" && -n "${STUB_MUSL_FAILS:-}" ]] && { echo "error: linking with cc failed" >&2; exit 101; }
d="$CARGO_TARGET_DIR/${t}release"; mkdir -p "$d"
if [[ "$p" == atlas-hub ]]; then
  printf "#!/usr/bin/env bash\necho \"Open this link to add a passkey (works once, for 15 minutes):\"\necho\necho \"  https://hub.example.ts.net:7443/setup/0123abcd\"\n" >"$d/atlas-hub"
else
  echo "atlasd ${t:-native}" >"$d/atlasd"
fi
chmod +x "$d/atlas-hub" "$d/atlasd" 2>/dev/null || true'
  for t in "$@"; do rm -f "$bin/$t"; done
}

# Runs install.sh in the current world; sets $out and $status. Extra args: VAR=value.
run() {
  status=0
  out="$(env -i HOME="$home" PATH="$bin" USER=tester STUB_LOG="$log" \
    XDG_STATE_HOME="$w/state" XDG_RUNTIME_DIR="$w/run" XDG_CONFIG_HOME="$home/.config" \
    GIT_CONFIG_NOSYSTEM=1 CARGO_TARGET_DIR="$w/target" "$@" \
    bash "$repo/deploy/install.sh" https://hub.example.ts.net:7443 </dev/null 2>&1)" || status=$?
}

check() { # description command...
  local what="$1"
  shift
  if "$@"; then
    echo "ok   $case_name: $what"
  else
    echo "FAIL $case_name: $what"
    printf '%s\n' "$out" | sed 's/^/     | /'
    failures=$((failures + 1))
  fi
}
says() { grep -qF -- "$1" <<<"$out"; }
failed() { ((status != 0)); }
no_build() { ! grep -q '^cargo' "$log"; }

refuses() { # case, phrase, world args, -- env args
  case_name="$1"
  local phrase="$2"
  shift 2
  local leave=()
  while (($#)) && [[ "$1" != -- ]]; do leave+=("$1") && shift; done
  (($#)) && shift
  world "$case_name" "${leave[@]}"
  run "$@"
  check "exits non-zero" failed
  check "says: $phrase" says "$phrase"
  check "nothing built" no_build
}

refuses not-linux "runs on Linux only" -- STUB_OS=Darwin
refuses no-cargo "cargo isn't installed" cargo
refuses no-git "git isn't installed" git
refuses no-tailscale "tailscale isn't installed" tailscale
refuses tailscale-signed-out "Tailscale isn't signed in" -- STUB_LOGGED_OUT=1
refuses no-user-bus "systemd user services aren't available" -- STUB_NO_USER_BUS=1
case_name=no-git-identity
world "$case_name"
rm "$home/.gitconfig"
run
check "exits non-zero" failed
check "asks for a git name and email" says "git has no name and email"
check "nothing built" no_build

case_name=fresh-home
world "$case_name"
run
k="$home/Knowledge"
check "succeeds" test "$status" -eq 0
check "lists what it will create first" says "This will:"
check "starter repo has one commit" test "$("$real_git" -C "$k" rev-list --count HEAD 2>/dev/null)" = 1
check "starter repo is on main" test "$("$real_git" -C "$k" branch --show-current 2>/dev/null)" = main
check "starter repo has the layout" test -f "$k/AGENTS.md" -a -d "$k/skills" -a -d "$k/memory" -a -d "$k/hooks"
check "tells you to push it" says "push -u origin main"
check "config lets in the Tailscale login" grep -q 'allowed_logins = \["you@example"\]' "$home/.config/atlas/hub.toml"
check "config has no public_skills" bash -c "! grep -q public_skills '$home/.config/atlas/hub.toml'"
check "config has no projects_dir" bash -c "! grep -q projects_dir '$home/.config/atlas/hub.toml'"
check "unit is a copy, not a link" test -f "$home/.config/systemd/user/atlas-hub.service" -a ! -L "$home/.config/systemd/user/atlas-hub.service"
check "atlasd build is where the hub serves it" test -x "$w/state/atlas-hub/dist/atlasd-linux-$(uname -m)"
check "says a native atlasd suits similar systems" says "similar or newer Linux"
check "shows tailscale serve's output" says "Available within your tailnet"
check "prints the setup link" says "/setup/0123abcd"
check "no Skills or Projects folders made" test ! -e "$home/Skills" -a ! -e "$home/Projects"

case_name=optional-folders
world "$case_name"
mkdir "$home/Skills" "$home/Projects"
run
check "config names ~/Skills" grep -q 'public_skills = "~/Skills"' "$home/.config/atlas/hub.toml"
check "config names ~/Projects" grep -q 'projects_dir = "~/Projects"' "$home/.config/atlas/hub.toml"

case_name=existing-knowledge
world "$case_name"
k="$home/Knowledge"
mkdir -p "$k/skills/mine"
echo "# Mine" >"$k/AGENTS.md"
echo "x" >"$k/skills/mine/SKILL.md"
"$real_git" -C "$k" init -q -b main && "$real_git" -C "$k" add -A
HOME="$home" "$real_git" -C "$k" commit -q -m mine
before="$(cd "$k" && find . -type f -print0 | sort -z | xargs -0 sha256sum)"
run
check "succeeds" test "$status" -eq 0
check "knowledge repo left byte-for-byte alone" test "$(cd "$k" && find . -type f -print0 | sort -z | xargs -0 sha256sum)" = "$before"
check "notes the missing upstream" says "has no upstream branch"

case_name=existing-config
world "$case_name"
mkdir -p "$home/.config/atlas" "$home/Knowledge"
printf 'allowed_logins = ["someone@else"]\nknowledge = "~/Knowledge"\npublic_url = "https://hub.example.ts.net:7443"\n' >"$home/.config/atlas/hub.toml"
before="$(cat "$home/.config/atlas/hub.toml")"
run
check "succeeds" test "$status" -eq 0
check "config untouched" test "$(cat "$home/.config/atlas/hub.toml")" = "$before"
check "says it keeps the config" says "keep your existing"

case_name=serve-fails
world "$case_name"
run STUB_SERVE_FAIL=1
check "exits non-zero" failed
check "shows Tailscale's own message" says "Serve is not enabled on your tailnet"
check "retried with sudo" grep -q '^sudo tailscale serve' "$log"

case_name=service-inactive
world "$case_name"
run STUB_INACTIVE=1
check "exits non-zero" failed
check "says the service isn't running" says "isn't running"
check "shows the journal" says "allowed_logins is empty"
check "no setup link" bash -c "! grep -q /setup/ <<<\"\$0\"" "$out"

case_name=no-linger
world "$case_name"
run STUB_NO_LINGER=1
check "still succeeds" test "$status" -eq 0
check "warns the hub stops at log out" says "the hub stops when you do"

case_name=musl-fails
world "$case_name"
run STUB_MUSL=1 STUB_MUSL_FAILS=1
check "succeeds" test "$status" -eq 0
check "tried the static build" grep -q -- '--target .*-unknown-linux-musl' "$log"
check "falls back to a native build" says "static build didn't link"
check "native build is served" grep -q native "$w/state/atlas-hub/dist/atlasd-linux-$(uname -m)"

case_name=musl-works
world "$case_name"
run STUB_MUSL=1
check "succeeds" test "$status" -eq 0
check "static build is served" grep -q musl "$w/state/atlas-hub/dist/atlasd-linux-$(uname -m)"

case_name=bad-url
world "$case_name"
status=0
out="$(env -i HOME="$home" PATH="$bin" STUB_LOG="$log" bash "$repo/deploy/install.sh" hub 2>&1)" || status=$?
check "exits 2" test "$status" -eq 2
check "suggests this machine's address" says "https://hub.example.ts.net:7443"

# --- Prebuilt release download paths -------------------------------------------------

run_get() {
  status=0
  out="$(env -i HOME="$home" PATH="$bin" USER=tester STUB_LOG="$log" \
    XDG_STATE_HOME="$w/state" XDG_RUNTIME_DIR="$w/run" XDG_CONFIG_HOME="$home/.config" \
    GIT_CONFIG_NOSYSTEM=1 "$@" \
    sh "$repo/deploy/get.sh" https://hub.example.ts.net:7443 </dev/null 2>&1)" || status=$?
}

case_name=download-without-cargo
world "$case_name" cargo
rel="$w/release"
make_release "$rel"
run STUB_RELEASE_DIR="$rel"
check "succeeds without cargo" test "$status" -eq 0
check "did not build" no_build
check "hub binary installed" test -x "$home/.local/bin/atlas-hub"
check "all four atlasd builds installed" test \
  -x "$w/state/atlas-hub/dist/atlasd-linux-x86_64" \
  -a -x "$w/state/atlas-hub/dist/atlasd-linux-aarch64" \
  -a -x "$w/state/atlas-hub/dist/atlasd-macos-x86_64" \
  -a -x "$w/state/atlas-hub/dist/atlasd-macos-aarch64"
check "prints the setup link" says "/setup/0123abcd"

case_name=download-flag-skips-cargo
world "$case_name"
rel="$w/release"
make_release "$rel"
status=0
out="$(env -i HOME="$home" PATH="$bin" USER=tester STUB_LOG="$log" \
  XDG_STATE_HOME="$w/state" XDG_RUNTIME_DIR="$w/run" XDG_CONFIG_HOME="$home/.config" \
  GIT_CONFIG_NOSYSTEM=1 CARGO_TARGET_DIR="$w/target" STUB_RELEASE_DIR="$rel" \
  bash "$repo/deploy/install.sh" --download https://hub.example.ts.net:7443 </dev/null 2>&1)" || status=$?
check "succeeds" test "$status" -eq 0
check "did not build" no_build
check "mentions downloading the hub" says "download the hub"

case_name=build-flag-ignores-release
world "$case_name"
rel="$w/release"
make_release "$rel"
status=0
out="$(env -i HOME="$home" PATH="$bin" USER=tester STUB_LOG="$log" \
  XDG_STATE_HOME="$w/state" XDG_RUNTIME_DIR="$w/run" XDG_CONFIG_HOME="$home/.config" \
  GIT_CONFIG_NOSYSTEM=1 CARGO_TARGET_DIR="$w/target" STUB_RELEASE_DIR="$rel" \
  bash "$repo/deploy/install.sh" --build https://hub.example.ts.net:7443 </dev/null 2>&1)" || status=$?
check "succeeds" test "$status" -eq 0
check "ran cargo" grep -q '^cargo' "$log"
check "did not curl" bash -c "! grep -q '^curl' '$log'"

case_name=fallback-no-release
world "$case_name"
run
check "succeeds by falling back to build" test "$status" -eq 0
check "explains why it fell back" says "No published release is available"
check "ran cargo" grep -q '^cargo' "$log"

case_name=download-bad-hub-checksum
world "$case_name" cargo
rel="$w/release"
make_release "$rel"
printf 'tampered\n' >>"$rel/atlas-hub-linux-$(uname -m)"
run STUB_RELEASE_DIR="$rel"
check "exits non-zero" failed
check "reports checksum mismatch" says "Checksum mismatch"
check "hub not installed" test ! -e "$home/.local/bin/atlas-hub"
check "config not written" test ! -e "$home/.config/atlas/hub.toml"
check "knowledge repo not created" test ! -e "$home/Knowledge"
check "dist not populated" test ! -e "$w/state/atlas-hub/dist"
check "tailscale serve not called" bash -c "! grep -q '^tailscale serve' '$log'"

case_name=download-bad-atlasd-checksum
world "$case_name" cargo
rel="$w/release"
make_release "$rel"
printf 'tampered\n' >>"$rel/atlasd-macos-aarch64"
run STUB_RELEASE_DIR="$rel"
check "exits non-zero" failed
check "reports checksum mismatch" says "Checksum mismatch"
check "hub not installed when atlasd fails" test ! -e "$home/.local/bin/atlas-hub"
check "dist not populated" test ! -e "$w/state/atlas-hub/dist"
check "service not written" test ! -e "$home/.config/systemd/user/atlas-hub.service"

case_name=get-sh-install
world "$case_name" cargo
rel="$w/release"
make_release "$rel"
run_get STUB_RELEASE_DIR="$rel"
check "succeeds without a clone or cargo" test "$status" -eq 0
check "hub binary installed" test -x "$home/.local/bin/atlas-hub"
check "all four atlasd builds installed" test \
  -x "$w/state/atlas-hub/dist/atlasd-linux-x86_64" \
  -a -x "$w/state/atlas-hub/dist/atlasd-linux-aarch64" \
  -a -x "$w/state/atlas-hub/dist/atlasd-macos-x86_64" \
  -a -x "$w/state/atlas-hub/dist/atlasd-macos-aarch64"
check "unit installed" test -f "$home/.config/systemd/user/atlas-hub.service"
check "prints the setup link" says "/setup/0123abcd"

case_name=get-sh-bad-checksum
world "$case_name" cargo
rel="$w/release"
make_release "$rel"
printf 'tampered\n' >>"$rel/atlas-deploy.tar.gz"
run_get STUB_RELEASE_DIR="$rel"
check "exits non-zero" failed
check "reports checksum mismatch" says "Checksum mismatch"
check "hub not installed" test ! -e "$home/.local/bin/atlas-hub"
check "config not written" test ! -e "$home/.config/atlas/hub.toml"

echo
if ((failures)); then
  echo "$failures checks failed"
  exit 1
fi
echo "All checks passed."
