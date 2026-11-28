# Atlas

One place for your complete agent setup: skills, instruction files, memory and hooks (MCP
servers next), carried to every machine, harness and login.

The hub runs on Linux. atlasd, the client, runs on Linux (x86_64 and ARM) and macOS
(Apple silicon and Intel).

- **Hub:** a web app, reached over your Tailscale network, that keeps everything in a git
  repo of your own. Edits need a passkey, and one tap saves them and sends them to your
  machines.
- **atlasd:** a small service on each machine that applies the signed updates to each
  agent's own config (Claude Code, Codex, OpenCode, Pi, Antigravity and others) and
  reports what it applied.

Atlas is early software by one developer. It works day to day, but expect breaking
changes between versions. Not yet built: the desktop app and MCP servers.

## You need

- A Linux machine with systemd for the hub, usually on.
- The Rust toolchain ([rustup](https://rustup.rs)) and `git`.
- A [Tailscale](https://tailscale.com) account, with HTTPS certificates turned on (admin
  console, DNS page). Passkeys only work over HTTPS.
- A browser that can use passkeys.
- A git remote for your knowledge repo that every machine can fetch from, such as a
  private GitHub repo. Machines fetch updates from it.

## Quick start

Clone the repo on the machine that will be the hub, then:

1. Run `deploy/install.sh` with the address the hub will have, such as
   `https://hub.example.ts.net:7443` (that machine's Tailscale name). It checks what's
   missing before building, says what it will create, sets up a starter knowledge repo
   if you have none, and prints a link to create your passkey. Then give the knowledge
   repo a remote and push it, as the script says.
2. On every machine whose agents Atlas should manage, the hub's own machine included:
   **Machines → Pair** in the hub, then run the command it shows.
3. Edit a skill in the hub and press Save. Your machines have it within a minute.

The [user guide](documents/guide/README.md) covers each step. The design is in
`documents/` (strategy, architecture, decisions) and setup files are in `deploy/`. The
public site (atlas.abdlh.com) is in `site/`.

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
deploy/test-install.sh   # the installer, in throwaway homes with stand-in system tools
```

## Licence, security and issues

MIT, see [LICENSE](LICENSE). Report security problems privately as
[SECURITY.md](SECURITY.md) describes. Bugs and ideas go to the
[issue tracker](https://github.com/n0mahd/atlas/issues).
