# Machines (pairing, install, SSH, unpair, status, CLI)

The owner pairs machines with one-time codes, installs atlasd with one line or over SSH,
watches what each machine reports, and manages machines from a workstation CLI
(decisions 0019, 0025, 0031).

## Routes
- `GET /machines`
- `GET /machines/list` (the list polls itself every 10 s)
- `POST /machines/pair`
- `POST /machines/pair/ssh`
- `POST /machines/add`
- `POST /machines/forget`
- `POST /machines/{id}/remove`
- `GET /install`
- `GET /install/{name}`
- `POST /api/pair`
- `POST /settings/tokens`
- `POST /settings/tokens/{id}/remove`
- `GET /api/admin/machines`
- `POST /api/admin/machines`
- `DELETE /api/admin/machines/{target}`
- `POST /api/admin/pair-code`

## Sub-features
- **Pair**: a one-time code (15 minutes) shown as `curl -fsSL <hub>/install | sh -s -- <code>`
  plus `ssh <target> '<command>'` (a machine's own Pair names it), each with Copy. A code
  works once; a wrong hub address is refused before the code is spent.
- **Track / forget**: add a name before pairing; the hub's own name and bad names are
  refused in words; forgetting a machine also unpairs its clients.
- **Unpair** a client (×): its token stops working; its files stay.
- **Status**: per machine "N paired", per client "Update N, up to date · seen just now" or
  "Update N, 1 behind", problems it reported, and one row per harness present
  (skills from Atlas / outside, instructions linked or imported, hooks, subagents).
- **Install script** `GET /install`: the hub's address and each build's SHA-256 filled in;
  `/install/atlasd-linux-<arch>` (GET) serves `<state>/dist/atlasd-linux-<arch>`; no build
  yet → "This hub has no atlasd builds to hand out yet."; other names 404.
- **Install over SSH** from the page (address field, or a tracked machine's Install) and
  `atlas-hub add-machine`: fresh code, `ssh -o BatchMode=yes -o ConnectTimeout=10 -o
  StrictHostKeyChecking=accept-new -- <target> "<command>"`; failures explained in plain
  words with the output's tail and the command offered for running by hand; bad addresses
  never reach ssh.
- **CLI**: Settings → CLI tokens (shown once; revocable) or `atlas-hub token <name>`;
  `atlasd auth`, `atlasd machines [list|track|remove|add]`, `atlasd pair-code`.

## How to get to it (owner's view)
Front page → V Machines. CLI tokens: Settings (top right) → III CLI tokens.

## Journeys
- `e2e/journeys/distribute/machines.spec.mjs`: `The open Machines page picks up a newly paired machine without a reload`.
- `e2e/journeys/distribute/machines.spec.mjs`: Pair shows a one-time install command that
  works once · Track a machine by name, then forget it · Unpair a client: its token stops
  working · Forgetting a tracked machine also unpairs its clients
- `e2e/journeys/distribute/install.spec.mjs`: before a build is published, /install says so ·
  the script carries the hub's address and the build's checksum · running the script
  installs, pairs and sets up the timer on a fresh machine
- `e2e/journeys/distribute/ssh.spec.mjs`: Install over SSH from the Machines page · A
  machine's own Install button uses its name as the address · A failed SSH install explains
  itself and offers the command to run by hand · atlas-hub add-machine installs over SSH
  from the hub's terminal
- `e2e/journeys/distribute/cli.spec.mjs`: a CLI token from Settings drives atlasd machines
  and pair-code · atlasd machines add installs over the workstation's own ssh · the admin
  API refuses calls without a CLI token
- Status and drift: `atlasd.spec.mjs` (laptop one update behind, then up to date).

## Evidence
- Screenshots: machines before pairing, pairing command, two machines paired, studio
  tracked / refused names / forgotten, spare paired / unpaired, installed over ssh, ssh
  install failed, bad ssh address refused, cli token created / revoked, machines: fresh
  installed by the script.
- Files on the fake machine: `~/.local/bin/atlasd` (mode 755, same SHA-256 as the build),
  `~/.config/atlas/client.toml` (mode 600, `machine`, `hub`, `adopt`),
  `~/.config/systemd/user/atlasd.{service,timer}` equal to `deploy/`, the shim logs
  `.fake-systemctl.log` / `.fake-loginctl.log`; `<world>/ssh.log` with the exact ssh argv.

## Gaps
- The install script calls `systemctl --user` and `loginctl enable-linger`; they are shims
  that only record their arguments, so the timer and the first `atlasd.service` start
  never run for real. The journey runs `~/.local/bin/atlasd run` by hand instead.
- The script can't name the machine; atlasd pairs under the hostname. The fake machine gets
  its name from a private UTS namespace (`unshare -r -u`); without unprivileged user
  namespaces the SSH and install journeys can't name machines.
- Real SSH (keys, Tailscale SSH, host-key prompts, the 3-minute limit) isn't driven: `ssh`
  is a fake that runs the command locally.
- Only the build for this machine's architecture is published and checked.
- Reloading Settings right after creating a CLI token resubmits the POST (no redirect after
  it), which would create a second token; the journey navigates instead.

## Gotchas
- `machine().atlasd(...)` in `world.mjs` is synchronous and blocks the event loop the proxy
  runs on: any atlasd command that talks to the hub then times out ("error sending request").
  Use the async `atlasd(m, args)` in `e2e/lib/distribute.mjs`. Same for running the install
  script or `atlas-hub add-machine` (they curl the proxy).
- Write fake executables with `writeExecutable` (a separate `sh` writes them) to avoid
  ETXTBSY, as in commit 9ff1304.
- The fake ssh needs the machine folder `<world>/machines/<host>` to exist (call
  `machine(world, host)` first); otherwise it answers like an unknown host (exit 255).
- An unpaired client's `atlasd run` says "couldn't reach the hub (the hub answered 401
  Unauthorized: Unknown client token.)" and exits 0.
