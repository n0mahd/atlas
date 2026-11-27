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
- Reading trusts the `Tailscale-User-Login` header from `tailscale serve` (decision 0012);
  writes need a passkey (0016). Any process running as the hub's user can reach the socket.
- Sessions and passkey ceremonies live in memory, so a hub restart signs the owner out.
- Pages share one read of the store for 3 seconds (saves through Atlas clear it), so a
  change made outside Atlas can take that long to show. A file watcher would remove the
  delay if it ever matters.
- The first-run window trusts any allowed Tailscale login while no passkey exists
  (decision 0018); the local app's first-run wizard takes onboarding over in phase 3.
- Saving runs `git` in project repos the owner also works in by hand; a save refuses when
  the file has other uncommitted changes, but can't stop the owner committing meanwhile.
- Links inside a repo are followed unless they point at a hidden file (`.env`, `.git`).
  Stricter would be "only files git tracks"; revisit when edits commit through Atlas.
- Harness folder paths in `atlas-harness` come from the existing `link-agents` script, not
  from each harness's docs. Check them when phase 3 writes to them.
