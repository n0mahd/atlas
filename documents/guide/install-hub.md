# Install the hub

The hub is the one machine that holds your setup and serves the Atlas web page. Pick a
machine that is usually on. You do this once.

## You need

- Linux with systemd.
- [Tailscale](https://tailscale.com), signed in, with HTTPS certificates turned on for
  your tailnet (admin console, DNS page). Passkeys only work over HTTPS.
- `git`, with your knowledge repo (for example `~/Knowledge`) checked out. It needs a git
  remote, such as a private GitHub repo: your machines fetch from there.
- The Rust toolchain (`rustup`). The repo pins the version in `rust-toolchain.toml`.

## The quick way

From a checkout of the Atlas repo, pick a port and run:

```sh
deploy/install.sh https://hub.example.ts.net:7443
```

Use your hub's own Tailscale name. The script builds the hub, writes
`~/.config/atlas/hub.toml` if there isn't one, starts the service, puts it on your
tailnet, and prints a one-time link to create your passkey. The steps below do the same
by hand.

## By hand

### 1. Build

```sh
cargo build --release -p atlas-hub
install -Dm755 target/release/atlas-hub ~/.local/bin/atlas-hub
```

### 2. Write the config

Copy `deploy/hub.example.toml` to `~/.config/atlas/hub.toml` and edit it:

```toml
allowed_logins = ["you@github"]       # your Tailscale login
knowledge = "~/Knowledge"
public_skills = "~/Skills"            # optional
projects_dir = "~/Projects"           # optional: repos here with agent files are projects
public_url = "https://hub.example.ts.net:7443"
```

`public_url` is the address you will open. Passkeys are tied to it, so pick it once.
Without it the hub is read-only.

### 3. Start the service

```sh
mkdir -p ~/.config/systemd/user
ln -sf "$PWD/deploy/atlas-hub.service" ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now atlas-hub
loginctl enable-linger "$USER"        # keep it running when you're logged out
```

The hub listens on a private socket, not a network port.

### 4. Put it on your tailnet

```sh
tailscale serve --bg --https=7443 "unix:$XDG_RUNTIME_DIR/atlas/hub.sock"
```

If Tailscale says you lack permission, run it with `sudo`.

## Create your passkey

While the hub has no passkey, it gives you a 30-minute window after it starts. Open your
`public_url` in a browser, signed in to Tailscale as the login in `allowed_logins`. Name
the passkey and tap **Create passkey**.

Missed the window, or lost your passkeys? On the hub run:

```sh
atlas-hub setup
```

It prints a link and a QR code that work once, for 15 minutes. Scan the QR code to add
your phone.

Add a second passkey in **Settings → Passkeys** (a phone or a security key), so losing
one device doesn't lock you out.

## Give the hub atlasd builds to hand out

Machines install atlasd from the hub, so the hub needs a copy for each kind of CPU. Build
static binaries with [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) (it
needs `zig` installed):

```sh
cargo install cargo-zigbuild
rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
for arch in x86_64 aarch64; do
  cargo zigbuild --release -p atlas-client --target $arch-unknown-linux-musl
  install -Dm755 target/$arch-unknown-linux-musl/release/atlasd \
    ~/.local/state/atlas-hub/dist/atlasd-linux-$arch
done
```

For Macs, build on a Mac (Apple silicon shown; use `x86_64` for Intel) and copy the
result to the hub. atlasd links Apple's Security framework, so it can't be cross-built
from Linux:

```sh
cargo build --release -p atlas-client
scp target/release/atlasd <hub>:.local/state/atlas-hub/dist/atlasd-macos-aarch64
```

`~/.local/state/atlas-hub` is the hub's state folder (`state_dir` in the config changes
it). You only need the builds for the CPUs you use. Repeat this when atlasd changes;
machines get the new build the next time you install it on them.

## Check it

```sh
systemctl --user status atlas-hub
journalctl --user -u atlas-hub -n 20
```

Open the page. **Machines** should list the hub with the agents it found. Next:
[add a machine](add-a-machine.md).

## Add Atlas to your app launcher

Open the hub page in Chrome, Chromium, Brave or Edge and choose **Install Atlas** (the
install icon at the end of the address bar, or the browser menu's *Cast, save and share →
Install page as app*). The browser adds an Atlas launcher with the Atlas icon, opens it in
its own window, and keeps that window grouped under the same icon in your dock or task bar.

A launcher made by hand, such as Omarchy's *Install Web App*, gets the icon too, but it has
to guess the window's name from the browser. If its window shows a generic icon in the
dock, remove that launcher and use the browser's **Install Atlas** instead.
