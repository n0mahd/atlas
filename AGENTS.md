# Atlas: agent instructions

Read `documents/strategy.md` and `documents/architecture.md` before changing anything.
Decisions live in `documents/decisions.md`; add an entry when you make one.

- Rust workspace. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` for the crates you touched.
- Commits use the repo's n0mahd identity. Never commit real names, emails, hostnames
  or secrets: this repo may become public.
- Atlas never patches a harness (Claude Code, Codex, OpenCode, Pi, Amp, Muse, T3). It
  only writes each harness's own config files, and only the entries it manages.
- Secrets (MCP API keys, tokens) never go in git. They live in the hub's keystore.
- UI changes: prototype first, matching the approved design direction (classic,
  maximalist, serif; see `documents/design.md`).
