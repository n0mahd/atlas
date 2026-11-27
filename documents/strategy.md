# Strategy

## Purpose
One place to manage agent skills, instruction files, memory, hooks and MCP servers, and carry
them to every machine, harness and login, so the user maintains each thing once.

## Priorities (in order)
1. Nothing is lost: every change is a git commit and can be undone.
2. Harnesses keep working when the hub is down (local copies, not live reads).
3. Easy to review and edit: one UI for skills, instructions, memory, hooks
   and MCPs.
4. One project owns everything. No second tool alongside it (no Toolport).

## Invariants
- Git is the store and the history. Files stay plain markdown/TOML so agents and editors
  can still edit them directly.
- The client writes only entries it manages in a harness's config and leaves hand edits.
- Secrets never enter git or the client's rendered files except where a harness needs a
  token to reach the hub's MCP gateway.
- Project skills belong to their project repo; Atlas commits edits there (to `main`).

## Direction
Build order: (1) read-only hub, (2) editing, (3) client service and local
app, replacing `~/Knowledge/bin/sync` and `link-agents`, (4) MCP list, gateway and
keystore. Each phase must work on its own. The hub is not tied to one machine.

## Debt register
- Phase 1 trusts the `Tailscale-User-Login` header from `tailscale serve` instead of a
  passkey (decision 0012). Any process running as the hub's user can reach the socket.
  Pay off in phase 2, before the first write.
- Links inside a repo are followed unless they point at a hidden file (`.env`, `.git`).
  Stricter would be "only files git tracks"; revisit when edits commit through Atlas.
- Harness folder paths in `atlas-harness` come from the existing `link-agents` script, not
  from each harness's docs. Check them when phase 3 writes to them.
