# Install the hub

The hub is the one machine that holds your setup and serves the Atlas web page. Pick a
machine that is usually on. You do this once.

## You need

- Linux with systemd user services. The hub doesn't run on macOS or Windows, and
  containers and WSL often lack the user session it needs.
- [Tailscale](https://tailscale.com), signed in, with HTTPS certificates turned on for
  your Tailscale network, your "tailnet" (admin console, DNS page). Passkeys only work
  over HTTPS.
- `git`, with a name and email set (`git config --global user.name …` and `user.email`):
  every save is a commit.
- A git remote for your knowledge repo that every machine can fetch from, such as a
  private GitHub repo. Your machines fetch updates from there, not from the hub.
- The Rust toolchain ([rustup](https://rustup.rs)). The repo pins the version in
  `rust-toolchain.toml`.
- A browser that can use passkeys.

## The knowledge repo

Your setup lives in one git repo, usually `~/Knowledge`. The hub reads and commits to
it; machines fetch it from its remote. Everything in it is plain files:

| Path | What it holds |
|---|---|
| `AGENTS.md` | Instructions every agent gets |
| `skills/<name>/SKILL.md` | Skills, one folder each, with any files they use |
| `memory/**.md` | Memory pages your agents read and write |
| `hooks/<name>/` | Hooks: a `hook.toml` (event, matcher, which agents) and its script |
| `agents/<name>.md` | Subagents, for Claude Code |
| `atlas/placements.toml` | Written by Atlas: which agents get which skills, where that differs from the default |

Any of these can be missing. If you have no knowledge repo yet, the install script makes
a starter one in `~/Knowledge`, on branch `main`, with one commit. Then give it a remote
and push it, once:

```sh
git -C ~/Knowledge remote add origin git@github.com:you/knowledge.git
git -C ~/Knowledge push -u origin main
```

The `-u` matters: an update names the commit your branch's upstream points at, and
machines fetch exactly that. Keep the repo private; it holds your memory and
instructions.

## The quick way

From a clone of the Atlas repo, pick a port and run:

```sh
deploy/install.sh https://hub.example.ts.net:7443
```

Use your hub's own Tailscale name; run the script without an address and it suggests
one. Before building anything, it checks for cargo, git, a signed-in Tailscale and
systemd user services, and says in one sentence what's missing. Then it lists what it
will create and, when you run it from a terminal, asks once before going on.

It then:

1. builds the hub and installs it as `~/.local/bin/atlas-hub`,
2. builds atlasd for this machine's kind of CPU and puts it where the hub hands it out
   (see [atlasd builds](#give-the-hub-atlasd-builds-to-hand-out)),
3. makes the starter knowledge repo, if `~/Knowledge` doesn't exist,
4. writes `~/.config/atlas/hub.toml`, if there isn't one, letting in your Tailscale
   login,
5. installs and starts the user service, and sets it to keep running after you log out,
6. puts the hub on your tailnet with `tailscale serve`,
7. checks the service is running, and prints a one-time link to create your passkey.

If Tailscale prints a link to turn on HTTPS or Serve, open it and approve; the script
goes on once you have. Running the script again is safe: it keeps your config and your
knowledge repo as they are, rebuilds, and restarts the hub.

The steps below do the same by hand.

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
public_url = "https://hub.example.ts.net:7443"
# public_skills = "~/Skills"          # optional: a second, shareable skills repo
# projects_dir = "~/Projects"         # optional: repos here with agent files are projects
```

`tailscale whois $(tailscale ip -4)` shows your login, under User. `public_url` is the
address you will open. Passkeys are tied to it, so pick it once. Without it the hub is
read-only.

### 3. Start the service

```sh
install -Dm644 deploy/atlas-hub.service ~/.config/systemd/user/atlas-hub.service
systemctl --user daemon-reload
systemctl --user enable --now atlas-hub
loginctl enable-linger "$USER"        # keep it running after you log out
```

Copy the unit rather than linking it, so moving or deleting the clone doesn't stop the
hub. The hub listens on a private socket, not a network port.

### 4. Put it on your tailnet

```sh
tailscale serve --bg --https=7443 "unix:$XDG_RUNTIME_DIR/atlas/hub.sock"
```

If Tailscale says you lack permission, run it with `sudo`.

## Who can open it

Only logins in `allowed_logins` get in. Everyone else on your tailnet, including other
people's logins and devices with a Tailscale tag (tagged devices have no login), gets a
403 "Atlas is open only to its owner's Tailscale login". That's enough to read; every
change also needs your passkey.

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

Machines install atlasd from the hub, so the hub needs a build for each kind of machine.
It keeps them in `~/.local/state/atlas-hub/dist/` (the hub's state folder; `state_dir`
in the config changes it), named `atlasd-<os>-<cpu>`: `atlasd-linux-x86_64`,
`atlasd-linux-aarch64`, `atlasd-macos-x86_64`, `atlasd-macos-aarch64`. You only need the
ones for the machines you use.

The install script makes the one for the hub's own CPU. When the musl target is installed
(`rustup target add x86_64-unknown-linux-musl`, and `cargo-zigbuild` with `zig` if a
plain build doesn't link), that build is static and runs on any Linux machine with that
CPU. Otherwise it's built for the hub's own system and suits only machines with the same
CPU and a similar or newer Linux.

For other CPUs, build static binaries with
[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) (it needs `zig`):

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

Repeat this when atlasd changes; machines get the new build the next time you install it
on them.

## Check it

```sh
systemctl --user status atlas-hub
journalctl --user -u atlas-hub -n 20
```

Open the page. **Machines** should list the hub with the agents it found. Next:
[add a machine](add-a-machine.md), starting with the hub's own machine if you run agents
there.

To remove the hub later, see [removing the hub](add-a-machine.md#removing-the-hub).

## Add Atlas to your app launcher

Open the hub page in Chrome, Chromium, Brave or Edge and choose **Install Atlas** (the
install icon at the end of the address bar, or the browser menu's *Cast, save and share →
Install page as app*). The browser adds an Atlas launcher with the Atlas icon, opens it in
its own window, and keeps that window grouped under the same icon in your dock or task bar.

A launcher made by hand, such as Omarchy's *Install Web App*, gets the icon too, but it has
to guess the window's name from the browser. If its window shows a generic icon in the
dock, remove that launcher and use the browser's **Install Atlas** instead.
