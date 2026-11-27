# Atlas

One place to manage agent skills, instruction files, memory, hooks and MCP servers, and carry
them to every machine, harness and login.

- **Hub:** a web app (reached over Tailscale) that holds everything in git and runs the
  MCP gateway.
- **Client:** a small service on each machine that writes each harness's own config
  (Claude Code, Codex, OpenCode, Pi and others) and reports what it applied.

Status: phase 2. The hub shows and edits skills, instructions and memory; every save is a
git commit, and changes need a passkey. It shows every skill, instruction file, memory page and
hook in the store, and what each harness on the hub machine reads. Setup is in `deploy/`. The public site (atlas.abdlh.com) is in `site/`.

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```
