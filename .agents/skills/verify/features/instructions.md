# Instructions

## Routes
- `GET /instructions`
- `GET /instructions/{scope}/{file}`
- `GET /instructions/{scope}/{file}/edit`
- `POST /instructions/{scope}/{file}/edit`
- `GET /instructions/{scope}/{file}/{action}`
- `POST /instructions/{scope}/{file}/{action}`
- `POST /instructions/claude-agents-md`
- `POST /send/start`
- `POST /send/finish`

## Sub-features
- List: Global section (`~/Knowledge/AGENTS.md`, line count, marks for agents reading it, a
  "Not used by <agents>" flag for installed agents that don't); with a project picked,
  "This project" first with the repo's AGENTS.md / CLAUDE.md ("No agent reads it", "X reads
  the other file").
- File page: rendered text, file path, per-agent state (linked from, imported by, uses its
  own, not set up here). 404 for unknown scopes or files.
- Edit and Save: one commit (`Edit global AGENTS.md in Atlas` / `Edit shop AGENTS.md in
  Atlas`); global saves send.
- Changes, each confirmed on its own page (`GET …/{action}`) and made as one change
  (`POST …/{action}`): `use-global` (an agent's own home file's text goes into Global and
  the agent reads Global), `to-agents` (a project's CLAUDE.md becomes or merges into its
  AGENTS.md), `to-global` (move a project file's text into Global), `from-global` (copy
  Global into this project), `create`. Unknown actions are 404; no file is read before
  the session is checked.
- Claude's option (`POST /instructions/claude-agents-md`): Claude Code reads AGENTS.md,
  stored as `[instructions] claude-reads-agents-md` in placements.toml for every machine;
  turning it on offers to send.

## How to get to it (owner's view)
Rail **Instructions** → a row → pencil → **Save**. **Showing** → project for its files.

## Journeys
`e2e/journeys/catalog/instructions.spec.mjs`:
- the list shows the global file, and Showing adds the project's
- each file has its own page
- saving the global file commits it to Knowledge and sends it
- saving a project's file commits it to the project's main
- the list says which agents don't use the global file (Claude Code linked to the store,
  Codex installed without it, then both linked)

## Evidence
Screenshots; git commit subject, single file, text at HEAD, Knowledge remote after send.

## Gaps
- The changes and Claude's option are driven by HTTP tests through the router with real
  git (`crates/atlas-web/tests/instructions.rs`) and the review-hub walkthrough; their
  browser journeys are being written on branch `feature-map`.
- A project's CLAUDE.md is not seeded, so "reads the other file" isn't driven.

## Gotchas
- The agents journey creates `~/.claude` and `~/.codex` in the hub's sandbox home and
  removes them afterwards; other files on the same worker see them for that test only.
