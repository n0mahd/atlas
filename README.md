# Atlas

One place to manage agent skills, instruction files, memory, hooks and MCP servers, and carry
them to every machine, harness and login.

- **Hub:** a web app (reached over Tailscale) that holds everything in git and runs the
  MCP gateway.
- **Client:** a small service on each machine that writes each harness's own config
  (Claude Code, Codex, OpenCode, Pi and others) and reports what it applied.

Status: phase 1, a read-only hub. It shows every skill, instruction file, memory page and
hook in the store, and what each harness on the hub machine reads. Setup is in `deploy/`.

```sh
cargo fmt && cargo clippy --all-targets -- -D warnings && cargo test
```
