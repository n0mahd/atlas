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

## 0011 · 2026-11-27 · One `hook.toml` per hook
A hook is a folder (`hooks/<name>/` in the knowledge repo, `.agents/hooks/<name>/` in a
project) holding its script and a `hook.toml`: `description`, `event` (Atlas's own names:
`session-start`, `user-prompt`, `pre-tool-use`, `post-tool-use`, `stop`), optional
`matcher`, `run` (a file in the folder, never a path out of it), optional `timeout` and
optional `harnesses`. Each harness adapter maps the event to the harness's own name and
skips harnesses that lack it. TOML because it's strict and already used for `mcp.toml`.

## 0012 · 2026-11-27 · Phase 1 access through `tailscale serve`
The hub listens only on a Unix socket (mode 0600 in a 0700 folder). `tailscale serve`
gives it HTTPS on the ts.net name and sets `Tailscale-User-Login`; the hub admits only
the logins in its config. Passkey sign-in, CSRF tokens and cookies arrive with editing in
phase 2, when there is something to protect beyond reading. Recorded as debt.

## 0013 · 2026-11-27 · The fifth harness is Antigravity
The Google harness Atlas manages is Antigravity (`antigravity`), not the Gemini CLI. Its
folders come from T3 Code's Antigravity driver: skills in `~/.gemini/config/skills` and
`~/.gemini/antigravity-cli/skills`, instructions in `~/.gemini/config/AGENTS.md`, project
skills in `.gemini/skills`, `.agents/skills` and `.agent/skills`.

## 0014 · 2026-11-27 · Screens follow the T3 Skills information design
Skills, Instructions and Memory show one scope at a time ("Showing [Global or a project]
on [machine]"); a project scope lists the project's items first, then Global. Rows are one
line plus a truncated description. Agents appear as brand marks only when they have the
item, or as one gilt sparkles mark when every agent does; detail opens inline. Machines
list machines, each with a row per agent. Project memory is the agent's own per-repo notes
(Claude Code's `~/.claude/projects/<repo>/memory`), shown read-only.

## 0015 · 2026-11-27 · Flat engraved Atlas medallion logo
Replaced the placeholder SVG armillary sphere with a flat 2D maximalist engraved medallion
inspired by the crouching verdigris Atlas statue holding a gilt armillary sphere inside a
Greek-key ring with acanthus and star corners. Master multi-size PNGs live in `assets/logo/`
(for desktop/Tauri packaging and launcher icons); `atlas-web` embeds quantized web assets
at `crates/atlas-web/static/logo.png` and `favicon.png`.

## 0016 · 2026-11-27 · Editing behind passkeys; commit, push optional
Skills, store memory notes and instruction files can be edited in the hub. The owner chose
passkeys before the first write (replacing 0012's Tailscale-only access for changes):
reading still needs only the owner's Tailscale login, while any change also needs a passkey
session (`__Host-` cookie, `SameSite=Strict`), its CSRF token and a same-origin request. The
first passkey is enrolled with a one-time setup code the hub writes to its own log.
Each save checks the file hasn't changed since it was opened and has no other uncommitted
changes, writes it atomically inside the repo (never through a link), and commits exactly
that file with hooks disabled; a failed commit restores the old text. Project repos must be
on `main` (0003). Saving commits only by default; a setting pushes after each save, except
the public skills repo, which is always pushed by hand after a privacy check. Agents' own
per-repo memory stays read-only because it isn't in git.

## 0017 · 2026-11-27 · Public site is static, on Cloudflare
The landing page (`site/`) is one static HTML page in the hub's design language, with no
script. It leads with the problem (each agent keeps its own copy of the setup, so copies
drift), then shows the fix. It is served by the Cloudflare Worker `atlas-site` (assets
only) at atlas.abdlh.com, so it never depends on a home machine. Deploy: `npx wrangler
deploy` from `site/` with Cloudflare credentials in the environment. Fonts, grain, key and
logo are copies of `atlas-web`'s; change both together. The zone caches static files for
hours, so rename an asset rather than editing it in place.
