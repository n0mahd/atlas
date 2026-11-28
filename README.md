# Atlas

One place for your complete agent setup: skills, instruction files, memory and hooks (MCP
servers next), carried to every machine, harness and login.

- **Hub:** a web app (reached over Tailscale) that holds everything in git. Edits need a
  passkey, and one tap saves and sends them to your machines.
- **atlasd:** a small service on each machine that applies the signed updates to each
  harness's own config (Claude Code, Codex, OpenCode, Pi, Antigravity and others) and
  reports what it applied.

Status: phase 3, the client. The hub shows and edits skills, instructions and memory; every
save is a git commit and needs a passkey. Save sends a signed update, and atlasd applies it
on each paired Linux machine every 10 minutes. Not yet: the local desktop app, macOS, and
MCP servers (phase 4).

## Quick start

1. Install the hub on one machine: `deploy/install.sh https://hub.example.ts.net:7443`,
   create your passkey in the browser, and give the hub atlasd builds to hand out.
2. On each other machine: **Machines → Pair** in the hub, then run the command it shows.
3. Edit a skill in the hub and press Save. Your machines have it within 10 minutes.

The [user guide](documents/guide/README.md) covers each step. The design is in
`documents/` (strategy, architecture, decisions) and setup files are in `deploy/`. The
public site (atlas.abdlh.com) is in `site/`.

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```
