# atlasd (apply, dry run, hand edits kept, every harness)

atlasd pulls the latest signed update, checks it, fetches the pinned trees and writes only
the entries it manages in each present harness (decisions 0020, 0023, 0026, 0028).

## Routes
- `POST /api/pair`
- `GET /api/release`
- `POST /api/report`
- `GET /machines`

## Sub-features
- `atlasd pair <hub> <code> --machine NAME` (config mode 600), `atlasd run --dry-run`
  (writes nothing; before the first run there are no local copies to plan from),
  `atlasd run`, `atlasd status` (hub, machine, last applied update, trusted passkeys, plan).
- A harness counts as present when its folder exists (`Layout::marker`): `~/.claude`,
  `~/.codex`, `~/.config/opencode`, `~/.pi/agent`, `~/.gemini`, `~/.config/amp`,
  `~/.config/muse`, `~/.grok`.
- Skill links in one folder per harness, in layout order: `~/.claude/skills`,
  `~/.agents/skills` (Codex), `~/.pi/agent/skills`, `~/.gemini/config/skills`; OpenCode,
  Amp, Muse and Grok Build read an already linked folder and get none of their own.
- Instructions: `@<copy>/AGENTS.md` appended once to the owner's `~/.claude/CLAUDE.md`;
  links for `~/.codex/AGENTS.md`, `~/.config/opencode/AGENTS.md`, `~/.pi/agent/AGENTS.md`,
  `~/.gemini/config/AGENTS.md`, `~/.config/amp/AGENTS.md`, `~/.config/AGENTS.md`,
  `~/.config/muse/AGENTS.md`, `~/.grok/AGENTS.md`.
- Hooks: a `# atlas:<name>` matcher group in `~/.claude/settings.json` and
  `~/.codex/hooks.json`; an `atlas:<name>` key in `~/.gemini/config/hooks.json`;
  `~/.grok/hooks/atlas.json` only when Claude Code isn't set up; none for Muse, OpenCode, Pi.
- Subagents: `~/.claude/agents/<name>.md` links (Claude Code only).
- Owner's files kept: settings.json minus Atlas's group is byte-for-byte the owner's;
  owner keys in Antigravity's hooks.json, Grok's own hook files, Muse settings, OpenCode
  `opencode.jsonc` (comments), Codex `config.toml` unchanged; an own skill folder at a
  managed name stays and is reported.
- Idempotent second run; drift (a removed link) shows in a dry run and is repaired by a run.
- Hub down: files and state unchanged, "couldn't reach the hub (...); harnesses keep using
  the local copies (release N)", exit 0; after the hub is back the next run applies.
- Reports: the Machines page shows each present harness's counts and the client's update.

## How to get to it (owner's view)
Machines → Pair → run the command on the machine; then `atlasd run`. Status shows on the
Machines page after each run.

## Journeys
`e2e/journeys/distribute/atlasd.spec.mjs`:
- laptop: pair, dry run, run, status: every harness gets its files and the owner's stay
- harnesses that read both Claude's and Codex's folders see each skill once
- box: only Codex, OpenCode and Grok Build; Grok gets its own hook file
- hub down: harness files stay, atlasd says why, and recovers when the hub is back

## Evidence
- Files under `<world>/machines/{laptop,box,laptop-offline}`: the links, `CLAUDE.md`, hook
  files, owner files compared byte for byte; `atlasd output` attachments.
- Screenshots: machines: laptop reported, laptop one update behind, box reported,
  machines after the hub came back, skill counts per harness.

## Gaps
- Host-scoped memory: atlasd places no memory (decision 0008: memory goes straight to git);
  the journey only checks `memory/hosts/laptop/` arrives in the pinned knowledge copy.
- The dry run fetches nothing, so right after pairing (as the install script suggests:
  "To see what it would change now: atlasd run --dry-run") it can only say "Nothing to plan
  without a local copy". It can't preview a new update either.
- Release refusal paths (bad signature, rollback, tree mismatch, key rotation) are covered
  by `crates/atlas-client/tests/run.rs`, not by a journey: they need a forged release.
- `adopt` roots (taking over links an older script made) aren't driven end to end.

## Gotchas
- Use the async `atlasd(m, args)` from `e2e/lib/distribute.mjs`, not `machine().atlasd`.
- Sessions live in the hub's memory: after `stopHub`/`startHub`, sign in again.
- With Claude Code and Codex both present, the multi-folder harnesses count every skill
  twice on the Machines page (the bug journey above).
