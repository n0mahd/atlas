# Strategy

## Purpose
One place for your complete agent setup: skills, instruction files, memory, hooks and MCP
servers, kept once in your own hub and carried to every machine, harness and login.
Subagent definitions are in scope for Claude Code (decision 0028); other harnesses'
subagents and slash commands are planned next.
Permissions and harness defaults (allowed tools, model, sandbox) need their own decision
first, because they are security-sensitive.

## Position
Tools like Toolport cover MCP servers, skills and rules for many harnesses through a local
gateway and a paid sync service. Atlas covers fewer harnesses but the whole setup, memory and
hooks included, with a self-hosted hub as the source of truth instead of a hosted service.

## Priorities (in order)
1. Nothing is lost: every change is a git commit and can be undone.
2. Harnesses keep working when the hub is down (local copies, not live reads).
3. Easy to review and edit: one UI for skills, instructions, memory, hooks
   and MCPs.
4. One project owns everything. No second tool alongside it (no Toolport).

## Invariants
Each with what enforces it today. "Convention" means nothing fails when it is broken.
- Git is the store and the history. Files stay plain markdown/TOML so agents and editors
  can still edit them directly. Convention.
- The client writes only entries it manages in a harness's config and leaves hand edits.
  Enforced by the golden tests in `crates/atlas-harness/tests/`.
- An update is applied only if the owner's passkey signed it, its number is higher than
  the last one applied and its content matches the signed tree hash. Enforced by the
  tests in `crates/atlas-verify/tests/` and `crates/atlas-client/tests/`.
- Secrets never enter git or the client's rendered files except where a harness needs a
  token to reach the hub's MCP gateway. Convention: there is no secrets code yet.
- Project skills belong to their project repo; Atlas commits edits there (to `main`).
  Enforced by tests in `crates/atlas-web`.
- `atlas-core` stays pure. Enforced by
  `crates/atlas-core/tests/dependency_allowlist.rs`.
- The docs describe the code. `crates/atlas-core/tests/architecture_map.rs` checks the
  crate tables and every repo path the docs name; the rest is convention.

## Direction
Build order: (1) read-only hub, (2) editing, (3) client service and local
app, replacing `~/Knowledge/bin/sync` and `link-agents`, (4) MCP list, gateway and
keystore. Each phase must work on its own. The hub is not tied to one machine.

## Debt register
Structure first, ranked by what it costs the next change (measured 2026-11-27 on the
skills work about to land):
1. Write logic lives in `atlas-web`: about 3,300 lines of saving, importing, placing,
   instruction moves, update checks and pairing sit in modules whose entry points take
   HTTP types, and `atlas-core` defines no ports. A second front end (Local Atlas, an MCP
   server) would have to copy them. Fix in steps: split each module into operations with
   no axum types and pages; then move the operations behind traits in an application
   crate; then build `atlas-testkit`.
2. Eleven source files are over 800 lines; `atlas-web`'s `views.rs`, `edit.rs` and
   `distribute.rs` are over 1,500 each and hold four or more jobs apiece.
3. Two commit pipelines (`edit.rs` and `change.rs`) each lock, check the branch, commit
   and restore on failure. One should remain.
4. One concept, several homes: the SSH target check (`atlas-web` and `atlas-client`),
   eight temp-file-and-rename writers, the owner-key type and its converter, and the
   session check that handlers call by hand instead of through an extractor.
5. `atlas-hub` has no tests (configuration, socket, first run), and the passkey
   ceremonies are tested only through the hub's pages.
6. Planned protections the architecture page lists as not built: a content scanner,
   `Secret<T>`, `VerifiedRelease`, Tailscale WhoIs, cargo-deny, signed binaries.

Then the standing items:
- Reading trusts the `Tailscale-User-Login` header from `tailscale serve` (decision 0012);
  writes need a passkey (0016). Any process running as the hub's user can reach the socket.
- Passkey ceremonies live in memory, so a hub restart in the middle of a sign-in means
  starting it again. Sessions are kept on disk (decision 0034).
- Pages share one read of the store for 3 seconds (saves through Atlas clear it), so a
  change made outside Atlas can take that long to show. A file watcher would remove the
  delay if it ever matters.
- The first-run window trusts any allowed Tailscale login while no passkey exists
  (decision 0018); the local app's first-run wizard takes onboarding over in phase 3.
- Saving runs `git` in project repos the owner also works in by hand; a save refuses when
  the file has other uncommitted changes, but can't stop the owner committing meanwhile.
- Links inside a repo are followed unless they point at a hidden file (`.env`, `.git`).
  Stricter would be "only files git tracks"; revisit when edits commit through Atlas.
- One hook script and matcher can't serve every harness: Antigravity names tools
  differently (`run_command`, not `Bash`) and sends different JSON. For now `harnesses` in
  `hook.toml` limits a hook; per-harness matchers would fix it properly.
- Atlas adds an `@` import to Claude's `CLAUDE.md` but has no way yet to remove it when a
  machine stops being managed.
- Machines install atlasd with `curl <hub>/install | sh -s -- <code>`; the hub serves
  builds it keeps in `<state>/dist/`, checked by SHA-256 in the script. `deploy/install.sh`
  makes the one for the hub's own CPU (static when the musl target is installed); other
  CPUs need `cargo zigbuild` by hand. They aren't signed yet: signed release builds from CI are the
  fix. macOS builds are made on a Mac (the TLS stack links Apple's Security framework) and
  run from a launchd agent instead of a systemd timer (decision 0033).
- Installing over SSH from the Machines page (decision 0025) runs inside the request, up to
  3 minutes, with no progress until it ends. A Tailscale SSH policy in `check` mode wants
  a browser sign-in that BatchMode can't give, so it only times out. A background job with
  streamed output would fix both if it matters.
- Harness folder paths in `atlas-harness` for Claude Code, Codex, OpenCode and Pi come from
  the existing `link-agents` script, not from each harness's docs (Amp, Muse and Herdr were
  checked, decision 0021; Grok Build, 0022). Check them when phase 3 writes to them.
- Skill placements (decision 0027): bringing in items seen only on other machines needs
  Local Atlas or a reviewed upload, since the hub reads only its own machine. A wanted
  agent can still get a skill from two folders it reads when the cheapest set of folders
  overlaps (OpenCode reads both `~/.claude/skills` and `~/.agents/skills`, as before
  0027); harnesses show the copy once by name, but it is untidy. `atlas/placements.toml`
  is rewritten by Atlas, so comments in it are lost.
- `atlas-web::change` checks each touched path with its own `git status`; a bulk change of
  hundreds of skills would want one batched status call. Fine for tens.

## Open decisions (the owner's)
- Should machines fetch each update's files from the hub instead of from a git remote?
  It would spare a new owner a private repository and git credentials on every machine;
  the remote would remain the backup and the way memory written on a machine comes back.
- When the write logic moves out of `atlas-web`, does it go into `atlas-core` or a new
  application crate? The allowlist test assumes the core stays free of I/O crates.

## Keeping this true
Change this page and `documents/architecture.md` in the same commit as the code that
moves a crate, a boundary or an invariant. `cargo test -p atlas-core` runs the map test,
and CI (`.github/workflows/ci.yml`) runs it on every push.
