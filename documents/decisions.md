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

## 0018 · 2026-11-27 · First passkey without a code
The log setup code from 0016 is gone. While a hub has no passkey, an allowed Tailscale login
can create the first one from the browser for 30 minutes after the hub starts
(`first_run_minutes`, 0 turns it off); finishing re-checks, so first run is claimed once.
Otherwise, and to recover lost passkeys, `atlas-hub setup` on the hub prints a link and QR
code that work once for 15 minutes; the state folder keeps only the token's hash. Running
it needs the hub machine, which is the trust the owner already has. Each passkey records
how it was added. `deploy/install.sh` installs the hub and opens a setup link.

## 0019 · 2026-11-27 · Releases, pairing and the client API
A release is a JSON manifest: a number, the hub's origin, each store repo's git remote,
commit and tree, and the owner's passkey public keys. The hub serialises it once; the
owner's passkey signs SHA-256 of those exact bytes as an ordinary WebAuthn assertion whose
challenge is the digest (user verification required). The hub checks the signature with
the same verifier clients use (`atlas-verify`) before storing it, and numbers are never
reused. Clients trust the keys they got at pairing, and a new key set only once a release
signed by an already-trusted key carries it. They fetch content from the git remotes,
which are untrusted storage: what counts is the pinned tree. Pairing: the signed-in owner
gets a one-time code (15 minutes) and runs `atlasd pair <hub> <code>` on the machine; the
client gets its own token (stored on the hub only as a hash). Clients call `/api/release`
and `/api/report` with that token, behind the same Tailscale login check as the UI.

## 0020 · 2026-11-27 · How Atlas writes harness files
`atlas-harness` holds one pure function per format (current bytes + desired entries → new
bytes), golden-tested on synthetic configs, with a facts table (`HARNESSES.md`) marking
each path as verified on an installed harness or taken from docs. Hooks: Claude Code
(`settings.json`) and Codex (`hooks.json`) get one matcher group per Atlas hook whose
command ends in a shell comment `# atlas:<name>`, so Atlas can find and update only its
own entries; Antigravity (`~/.gemini/config/hooks.json`) uses keys named `atlas:<name>`.
OpenCode and Pi have hooks only as code plugins, so Atlas doesn't write them. Atlas never
writes Codex's hook trust record: approving a new hook stays the owner's step in Codex.
Skill links and instruction files only ever change links into roots Atlas manages; a
user's own folder or file is reported as a conflict, never overwritten.

## 0021 · 2026-11-27 · Amp and Muse are harnesses; Herdr is a host
Atlas adds Amp (`amp`), Muse Code (`muse`) and Herdr (`herdr`). Paths are relative to the
login's home; "repo" means inside a project.
- **Amp**, from its manual (ampcode.com/docs: Skills, AGENTS.md, Configuration, MCP,
  Plugins; Nov 2026) and the CLI's strings. Skills, in Amp's precedence order:
  `~/.config/agents/skills` (where `amp skill add --global` installs), `~/.agents/skills`,
  `~/.config/amp/skills`, `~/.claude/skills`; repo `.agents/skills` then `.claude/skills`.
  Instructions: both `~/.config/amp/AGENTS.md` and `~/.config/AGENTS.md`; in a repo
  `AGENTS.md`, else `AGENT.md` or `CLAUDE.md`. MCP: `amp.mcpServers` in
  `~/.config/amp/settings.json` (or `.jsonc`) and the repo's `.amp/settings.json`, which
  needs approval; a skill can also carry an `mcp.json`. Hooks: none Atlas can render.
  Amp's event handlers are TypeScript plugins (`~/.config/amp/plugins/`, repo
  `.amp/plugins/`), which are code, not a command config. Memory: none local; personal and
  workspace skills live in Amp-hosted git repos outside Atlas. Not checked against a live
  install, because every Amp command needs a sign-in.
- **Muse Code** (Meta), checked against Muse Code 1.4.25 itself with a throwaway home: what
  `muse skills list --json` found, which probe files reached a session, what
  `muse mcp add` wrote, and which hooks ran. Skills: `~/.config/muse/skills`,
  `~/.agents/skills`, `~/.claude/skills`, `~/.codex/skills`; repo `.agents/skills`,
  `.claude/skills`, `.codex/skills`. Instructions: `~/.config/muse/AGENTS.md`; in a repo
  `AGENTS.md`, else `CLAUDE.md`. MCP: `mcpServers` in `~/.config/muse/settings.json`
  (entries carry `type: stdio` or a URL) and the repo's `.mcp.json`. Hooks: `hooks` in
  that same settings.json, in Claude Code's shape and event names, so the existing reader
  covers them. Memory: `MEMORY.md` indexes in three scopes; project memory is
  `<repo>/.agents/memory/`. Where the two personal scopes are kept was not established. An
  untrusted repo's rules, skills and MCP servers are skipped until the user trusts it.
- **Herdr** is a terminal workspace that runs the other agents (`herdr --help`, its
  `config.toml`), so it is modelled like T3 Code: a marker (`~/.config/herdr`) and no files
  of its own. Its `herdr integration install <agent>` adds a status hook to other
  harnesses' configs, which Atlas sees as hooks outside Atlas.

Marks: Amp's and Muse's one-colour icons from the Agent Client Protocol registry; Herdr gets
a monogram like T3 Code. `Layout` has no MCP or memory fields yet; phase 4 adds MCP paths.

## 0022 · 2026-11-27 · Grok Build is a harness
Atlas adds xAI's Grok Build (id `grok`, command `grok`). Its `--help`, docs.x.ai/build and
its ACP registry entry all call it Grok Build; "Grok Bot" is a different xAI product. Checked against Grok Build 1.0.92
with a throwaway home: what `grok inspect --json` found from probe files, with and without
folder trust, and what `grok mcp add` wrote. Paths are relative to the login's home.
- Skills: `~/.grok/skills`, `~/.agents/skills`, `~/.claude/skills`, `~/.cursor/skills`; in a
  repo the same four folders, walked up to the repo root. Linked skill folders load.
- Instructions: `~/.grok/AGENTS.md`, `Agents.md` and `CLAUDE.md`, every `*.md` in
  `~/.grok/rules/`, and Claude Code's `~/.claude/CLAUDE.md` and `~/.claude/rules/`. In each
  directory from the repo root down it loads every one of `AGENTS.md`, `Agents.md`,
  `AGENT.md`, `CLAUDE.md`, `Claude.md` and `CLAUDE.local.md`, not the first. Atlas links
  `~/.grok/AGENTS.md`, which a link satisfies. Whether Grok expands the `@` import Atlas
  puts in `CLAUDE.md` is unknown (that needs a signed-in session), so the global file may
  reach a session twice on a machine that also has Claude Code.
- Hooks: a folder, `~/.grok/hooks/*.json` (repo `.grok/hooks/`, trusted folders only), each
  file in Claude Code's shape, plus Claude Code's and Cursor's hook files. `Layout`'s
  `hook_config` may now name a folder; the reader reads its JSON files in name order. Atlas
  doesn't write there: Grok already runs the hooks Atlas puts in `~/.claude/settings.json`,
  and a second copy would run twice. A machine with Grok but no Claude Code, or a hook
  limited to `grok`, therefore gets no Atlas hook yet.
- MCP: `[mcp_servers.<name>]` in `~/.grok/config.toml` (`command`/`args` or `url`, with
  `enabled = true`), which `grok mcp add` writes; `--scope project` writes the repo's
  `.grok/config.toml`. It also loads `~/.claude.json`, `.cursor/mcp.json` and a repo's
  `.mcp.json`. OAuth tokens sit in `~/.grok/mcp_credentials.json`.
- Memory: off by default; when on, Markdown topics under `~/.grok/memory-v2/` (`global/`
  and `workspaces/<project>-<hash>/`). Docs only.

Mark: the one-colour `grok-build.svg` from xAI's Agent Client Protocol registry entry.

## 0023 · 2026-11-27 · How atlasd applies a release
`atlasd` (crate `atlas-client`) runs as each OS account, from a systemd user timer every
10 minutes, never as root. Each run: fetch the latest signed release, check it against the
keys trusted at pairing (or carried by an applied release), refuse rollbacks and reused
numbers, fetch every pinned repo into `~/.local/share/atlas/sources/` and check each tree
before checking any out, then plan and apply: skill links in one folder per harness (a
harness that already reads a linked folder gets nothing more, so no skill appears twice),
the global `AGENTS.md` linked or, for Claude Code, imported once in `CLAUDE.md` (an older
import pointing into a managed root is replaced, not duplicated), and Atlas's hooks in
each harness's own hook file. Only entries in roots Atlas manages are changed; `adopt`
roots in `client.toml` let it take over links made by earlier scripts. New keys and the
release number are saved only after everything applied. With the hub unreachable nothing
changes, so harnesses keep working from the local copies. On a machine running atlasd,
`~/Knowledge/bin/sync` must stop calling `link-agents`, or both would manage the same links.
