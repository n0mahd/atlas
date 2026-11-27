# Decisions

## 0001 · 2026-11-26 · One project owns everything
Atlas manages skills, instructions, memory and MCPs itself, MCP gateway included. No
Toolport or other tool alongside it; Toolport supplied ideas only. A forked T3 Code was
ruled out because custom builds broke stock updates. Obsidian alone was ruled out because
it edits only the local machine.

## 0002 · 2026-11-26 · Hub plus local copies
A hub holds the git store; each machine gets local copies so harnesses
keep working when the hub is down.

## 0003 · 2026-11-26 · Project skill edits commit to main
Edits to a project's skills are committed straight to that repo's `main`. Note: in repos
where `main` auto-deploys, an edit goes live at once.

## 0004 · 2026-11-26 · "Every login" means both
Every user account on every machine gets the setup (including service accounts that run agents),
and harness sign-ins are tracked too.

## 0005 · 2026-11-26 · Rust
Hub and client are Rust single binaries for x86_64 and aarch64. UI is server-rendered
(axum + askama + htmx) to keep one language.

## 0006 · 2026-11-26 · Design direction
Classic, maximalist, serif: Cinzel titles, EB Garamond body, IBM Plex Mono for paths and
status; limestone ground, ink, verdigris accent, gilt ornament, oxblood for problems;
Hermes Agent's structure (rules, spaced uppercase labels, numbered entries, grain),
not its colours. See `documents/design.md`.

## 0007 · 2026-11-27 · Hexagonal architecture; the user's passkey signs releases
A pure `atlas-core` with ports, adapter crates, and two composition roots (`atlas-hub`,
`atlasd`). Releases are signed by the user's passkey, not by a key the hub holds, so a
compromised hub cannot push changes to machines. The hub can run on any machine;
clients learn its address at pairing. A local desktop app (Tauri) edits local files and
sends global edits to the hub as proposals. See `architecture.md`.

## 0008 · 2026-11-27 · Memory goes straight to git
Agents keep writing memory directly to git; the scanner flags but doesn't block. A
setting lets a user require a release for memory too.

## 0009 · 2026-11-27 · Local stdio MCP servers only from a signed release
A machine starts a local (stdio) MCP server only if it arrived in a release the user
signed. Hub-only was ruled out because servers that need local files, a browser or git
couldn't work.

## 0010 · 2026-11-27 · Hooks are in scope
Atlas manages harness hooks alongside skills, instructions, memory and MCPs: one hook
definition, rendered into each harness's own hook config by its adapter, global or per
project. Hooks run commands on the machine, so like local MCP servers they reach a
machine only through a signed release, and the scanner flags every hook change.
