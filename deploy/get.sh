#!/bin/sh
# Bootstrap the Atlas hub without cloning the repository:
#
#   curl -fsSL https://raw.githubusercontent.com/n0mahd/atlas/main/deploy/get.sh | sh -s -- https://<this-machine>.<tailnet>.ts.net:<port>
#
# Downloads the deploy archive from the latest GitHub release, checks its SHA-256 against
# SHA256SUMS from that release, and runs deploy/install.sh --download.
set -eu

die() {
  printf '%s\n' "$*" >&2
  exit 1
}

[ "$(uname -s)" = Linux ] ||
  die "The Atlas hub runs on Linux only, so it can't be installed on this $(uname -s) machine. (atlasd, the client, also runs on macOS.)"
command -v bash >/dev/null 2>&1 || die "bash isn't installed."
command -v curl >/dev/null 2>&1 || die "curl isn't installed."
command -v tar >/dev/null 2>&1 || die "tar isn't installed."
if command -v sha256sum >/dev/null 2>&1; then
  verify() { printf '%s  %s\n' "$1" "$2" | sha256sum -c --quiet -; }
elif command -v shasum >/dev/null 2>&1; then
  verify() { printf '%s  %s\n' "$1" "$2" | shasum -a 256 -c --quiet -; }
else
  die "Neither sha256sum nor shasum is installed, so the download can't be verified."
fi

base="${ATLAS_RELEASE_URL:-https://github.com/n0mahd/atlas/releases/latest/download}"
archive="atlas-deploy.tar.gz"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

curl -fsSL --connect-timeout 10 --max-time 30 -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" ||
  die "Couldn't download SHA256SUMS from $base. Nothing was installed."

sum="$(awk -v f="$archive" '$2 == f && $1 ~ /^[0-9a-fA-F]{64}$/ { print $1; exit }' "$tmp/SHA256SUMS")"
[ -n "$sum" ] || die "SHA256SUMS has no entry for $archive. Nothing was installed."

curl -fsSL --connect-timeout 10 --max-time 60 -o "$tmp/$archive" "$base/$archive" ||
  die "Couldn't download $archive from $base. Nothing was installed."

verify "$sum" "$tmp/$archive" >/dev/null 2>&1 ||
  die "Checksum mismatch for $archive. Nothing was installed."

tar -xzf "$tmp/$archive" -C "$tmp"
[ -f "$tmp/deploy/install.sh" ] ||
  die "The deploy archive did not contain deploy/install.sh. Nothing was installed."

bash "$tmp/deploy/install.sh" --download "$@"
