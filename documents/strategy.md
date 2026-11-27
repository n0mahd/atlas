# Strategy

## Purpose
One place to manage agent skills, instruction files, memory and MCP servers, and carry
them to every machine, harness and login, so the user maintains each thing once.

## Priorities (in order)
1. Nothing is lost: every change is a git commit and can be undone.
2. Harnesses keep working when the hub is down (local copies, not live reads).
3. Easy to review and edit: one UI for skills, instructions, memory and MCPs.
4. One project owns everything. No second tool alongside it (no Toolport).

## Invariants
- Git is the store and the history. Files stay plain markdown/TOML so agents and editors
  can still edit them directly.
- The client writes only entries it manages in a harness's config and leaves hand edits.
- Secrets never enter git or the client's rendered files except where a harness needs a
  token to reach the hub's MCP gateway.
- Project skills belong to their project repo; Atlas commits edits there (to `main`).

## Direction
Build order: (1) read-only hub, (2) editing, (3) client service replacing
`~/Knowledge/bin/sync` and `link-agents`, (4) MCP list, gateway and keystore.
Each phase must work on its own.

## Debt register
(none yet)
