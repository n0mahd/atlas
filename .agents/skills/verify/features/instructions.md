# Instructions

## Routes
- `GET /instructions`
- `GET /instructions/{scope}/{file}`
- `GET /instructions/{scope}/{file}/edit`
- `POST /instructions/{scope}/{file}/edit`
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
- "Use Global instead" and moving a project's CLAUDE.md into AGENTS.md (docs/skills.md)
  have no UI yet.
- A project's CLAUDE.md is not seeded, so "reads the other file" isn't driven.

## Gotchas
- The agents journey creates `~/.claude` and `~/.codex` in the hub's sandbox home and
  removes them afterwards; other files on the same worker see them for that test only.
