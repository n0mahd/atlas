# Atlas: agent instructions

Orient first: read `documents/strategy.md` (what must not break, where it is heading,
the debt) and `documents/architecture.md` (its "Start here" table leads to the files)
before changing anything. Name the module that owns the change before you edit. If a
change moves a crate, a boundary or an invariant, update both pages in the same commit;
`cargo test -p atlas-core` fails when they name a crate or path that isn't there.
Decisions live in `documents/decisions.md`; add an entry when you make one, headed
`## 00XX · <date> · <title>` while other branches are open: the number is assigned at
merge, so two branches never claim the same one.

- Rust workspace. Run `cargo fmt`, `cargo clippy --all-targets -- -D warnings` and
  `cargo test` for the crates you touched.
- Commits use the repo's n0mahd identity. Never commit real names, emails, hostnames
  or secrets: this repo may become public.
- Atlas never patches a harness (Claude Code, Codex, OpenCode, Pi, Amp, Muse, Grok Build,
  T3). It only writes each harness's own config files, and only the entries it manages.
- Secrets (MCP API keys, tokens) never go in git. They live in the hub's keystore.
- UI changes: prototype first, matching the approved design direction (classic,
  maximalist, serif; see `documents/design.md`).
