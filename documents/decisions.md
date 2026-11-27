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
Every user account on every machine gets the setup (for example a separate `agent` account),
and harness sign-ins are tracked too.

## 0005 · 2026-11-26 · Rust
Hub and client are Rust single binaries for x86_64 and aarch64. UI is server-rendered
(axum + askama + htmx) to keep one language.

## 0006 · 2026-11-26 · Design direction
Classic, maximalist, serif: Cinzel titles, EB Garamond body, IBM Plex Mono for paths and
status; limestone ground, ink, verdigris accent, gilt ornament, oxblood for problems;
Hermes Agent's structure (rules, spaced uppercase labels, numbered entries, grain),
not its colours. See `documents/design.md`.
