# Harnesses see what atlasd wrote

The outcome check behind distribution: after a real Send and a real `atlasd run`, each
installed harness binary is asked, offline, what it actually loaded from the machine's
HOME: skills (and whether each is switched off), instruction files, hooks and subagents.
Not "the links look right" but "Claude Code lists the skill".

## Routes

This feature has no hub routes of its own; it drives Send and the client API.

- `GET /`
- `POST /send/start`
- `POST /send/finish`
- `GET /machines`
- `POST /api/pair`
- `GET /api/release`
- `POST /api/report`

## Sub-features

- Global skills from Knowledge and public skills (deploy, pdf-tools, review) are listed by
  every harness atlasd treats as present.
- The global `AGENTS.md` is loaded, where the harness can say which instruction files it
  read.
- The `no-rm` hook (pre-tool-use, Bash) is seen where Atlas writes hooks and the harness can
  show them offline; Claude Code also runs it.
- The `reviewer` subagent is listed by Claude Code, and by Grok Build, which reads
  `~/.claude/agents`.
- A skill switched off for some agents in `atlas/placements.toml` is gone (or reported off)
  for those agents only, after the next Send and `atlasd run`.
- A project checkout (`~/Projects/shop`, cloned from its remote) gets its own `stock` skill
  in every harness run inside it.

## How to get to it (owner's view)

Front page → **Needs a look** → **Send** (passkey tap). On the machine: `atlasd pair <hub>
<code> --machine laptop`, then `atlasd run`. Machines shows the laptop "Update 1, up to
date" with a row per harness. Per-agent switches have no control in the hub yet
(`skills_place::set_agents` refuses), so the owner edits `atlas/placements.toml` in
Knowledge, commits, and the front page offers Send again.

## Journeys

`e2e/journeys/harnesses/harnesses.spec.mjs` (desktop only: probes don't depend on the
viewport; phone skips with that reason). Probes are in `e2e/lib/harnesses.mjs`
(`probe(name, { home, cwd, marker, check, trust })`, `probeAll`).

- the owner sends and a laptop with every harness applies it
- every harness lists the global skills
- instructions, hooks and subagents are seen where each harness reports them
- Claude Code runs the no-rm hook cleanly (`test.fail`: bug, below)
- a skill switched off for some agents is gone for them only (`test.fail`: bug, below)
- a project checkout's own skill shows inside it (`test.fail`: bug, below)
- every harness atlasd knows is probed (keeps the probes in step with `atlas-harness` layouts)

## Per harness

Every probe runs the real binary (resolved with `mise which` under the real HOME, linked
into `target/verify/<run>/harness-bin`) with the machine's sandbox env, no API keys, a hard
timeout, and under `unshare -rn` (no network) except Claude Code. Home-level probes run in a
fresh git folder outside the repo and the real HOME.

| Harness | Proven | Probe |
|---|---|---|
| Claude Code | skills, subagent `reviewer`, `~/.claude/CLAUDE.md` and the imported store `AGENTS.md`, the PreToolUse:Bash hook firing | `claude -p /context --output-format stream-json --verbose` with `ANTHROPIC_BASE_URL=http://127.0.0.1:9` (init line `skills`, `agents`; `memory_files`); then `claude -p … --include-hook-events` against a fake Messages API on loopback that asks for one Bash call (`hook_started`/`hook_response` `PreToolUse:Bash`) |
| Codex | skills with `enabled`, the `no-rm` hook (`# atlas:no-rm`), the instructions' text | `codex app-server` JSON-RPC `initialize`, `skills/list`, `hooks/list`; `codex debug prompt-input` holds the global AGENTS.md line |
| OpenCode | skills; switch via the build agent's `skill` deny rules | one shell in the namespace: `opencode serve` + `opencode api … /api/skill` polled until it settles, then `opencode debug agents` |
| Pi | skills, `~/.pi/agent/AGENTS.md` and the text in the prompt | `pi --mode rpc --offline --no-session -e <probe extension>`: `get_commands` (`skill:<name>`), then `/atlas-probe` writes `contextFiles` |
| Amp | skills; switch via `amp permissions test skill` | `amp skill list --json`, `amp permissions test skill --json --name <skill>` (dummy `AMP_API_KEY`) |
| Muse | skills with `activation`, `~/.config/muse/AGENTS.md` | `muse skills list --json`; `muse exec --provider echo --json hi` then the session's `session.jsonl` `<rules-file …>` |
| Grok Build | skills with `disabled`, `.grok/AGENTS.md` (and `.claude/CLAUDE.md`), the `no-rm` hook via Claude's settings, subagent `reviewer` | `grok inspect --json` |
| Antigravity | skills | `agy acp` JSON-RPC `initialize`, `authenticate` (`gemini-api-key`, dummy key), `session/new`; `available_commands_update` |

## Evidence

- Screenshots: front page before Send, after Send, Machines with the laptop up to date, the
  placements change waiting, and sent.
- Per probe a text attachment `<harness> · <step>.txt`: the parsed summary and what the
  harness itself printed. Each `atlasd run` output is attached too.
- Side effects: `atlasd run` says `Release 1 is applied` / `Release 2 is applied`; Knowledge
  isn't ahead of its remote after Send.

## Bugs found (kept as `test.fail` with a `bug` annotation)

- atlasd ignores `atlas/placements.toml`: nothing in `crates/atlas-client` reads it or writes
  a switch, so `pdf-tools` switched off for claude-code, codex and pi is still listed and on
  in all three (decision 0027, `documents/skills.md`).
- Projects never reach machines: `release::build` leaves projects out of updates and atlasd
  looks for no checkouts, so Claude Code (which reads only `.claude/skills` in a repo) doesn't
  list shop's `stock`. Codex, OpenCode, Pi, Amp, Muse, Grok Build and Antigravity read
  `.agents/skills` themselves and do.
- A hook script without the execute bit is installed silently: atlasd checks only that the
  script exists and writes its bare path as the command, so Claude Code's run fails with
  "Permission denied" (exit 126). The seed's `check.sh` is committed 100644.

## Gaps

- Instruction files: OpenCode, Amp and Antigravity have no offline way to say which they
  loaded. Codex shows the text, not the path.
- Hooks: Antigravity shows hooks only by running them, and offline only SessionStart runs
  (`no-rm` is pre-tool-use). Atlas writes no hooks for Muse, though its layout names
  `~/.config/muse/settings.json` (`plan.rs` renders only Claude, Grok, Codex and
  Antigravity). OpenCode, Pi and Amp take hooks as plugins, which Atlas doesn't write.
- Switches: no known way to switch a skill off in Antigravity, so it isn't checked there.
- Amp (`amp.permissions` reject rules), Muse (`skills.activation` in settings.json) and Grok
  Build (`[skills] disabled` in `~/.grok/config.toml`) have per-skill switches Atlas doesn't
  use yet (`atlas_harness::switches` covers Claude Code, Codex, OpenCode and Pi only).
- Codex lists Atlas's hook as `trustStatus: untrusted`; it won't run until the owner trusts
  it in Codex. Grok Build marks it `waitsForFolderTrust`. Neither is driven.
- Hooks running in harnesses other than Claude Code would need a model turn each; only
  Claude Code has a fake model here.

## Gotchas

- `machine().atlasd` is synchronous and blocks the in-process proxy, so `atlasd pair` fails
  with "error sending request"; use `atlasd(m, …)` from `e2e/lib/harnesses.mjs`.
- mise shims re-download under a sandbox HOME; binaries are resolved with the real HOME
  first. OpenCode and Amp are node scripts, so `node` is linked too.
- Worlds live under the real HOME, and harnesses walk up from the cwd: Amp picked up the real
  `~/.claude/skills`, Claude Code the real `~/.claude/CLAUDE.md`, Pi the repo's `AGENTS.md`.
  Home-level probes therefore run in a fresh git folder under `$XDG_RUNTIME_DIR`.
- Pi, Muse and Grok Build load a project's skills only once the folder is trusted. Project
  probes trust it for that run only (`pi --approve`, `muse --trust-workspace`,
  `GROK_FOLDER_TRUST=0`), as the owner would at the trust prompt.
- OpenCode's `/api/skill` shows only built-ins for the first seconds; the probe polls until
  the list stops changing. `opencode debug agents` also needs loopback.
- Without `AMP_API_KEY` Amp starts a sign-in flow and hangs; a dummy key is never used offline.
- Machines counts 6 skills for OpenCode, Amp, Muse and Grok Build (they read both
  `~/.claude/skills` and `~/.agents/skills`), but each harness lists every skill once.
