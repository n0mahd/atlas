# Hooks

## Routes
- `GET /hooks`

## Sub-features
- Read-only. **In Atlas**: each `hooks/<name>/hook.toml` with description, event in words,
  "Script missing" flag, and a drawer with scope, tools (matcher), runs (script · timeout),
  agents and file.
- **Outside Atlas**: hooks found in agents' own configs on the hub machine (e.g. Claude
  Code's `~/.claude/settings.json`), with the agent's mark, command, event · matcher, config
  path and timeout; "No agent on this machine has hooks of its own." when none.
- No editor.

## How to get to it (owner's view)
Rail **Hooks** → expand a row.

## Journeys
`e2e/journeys/catalog/hooks.spec.mjs`:
- hooks in Atlas show their manifest; none outside
- a hand-made agent hook shows outside Atlas

## Evidence
Screenshots of both sections and the open drawers; the running head counts
("1 in Atlas · 1 outside").

## Gaps
- Editing hooks, project hooks (`.agents/hooks`), the "Script missing" flag, and
  **Bring into Atlas** aren't driven (no UI for the last; no seeded broken hook).
- What atlasd installs from a hook is the client area's journey.

## Gotchas
- The outside-hook journey writes `~/.claude/settings.json` in the hub's sandbox home and
  removes it; wait 3 s for the hub's cache either side.
