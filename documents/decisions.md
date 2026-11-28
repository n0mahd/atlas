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

## 0024 · 2026-11-27 · Save sends; updates instead of releases in the UI
Non-technical owners never meet the word "release": the UI says "update", and there is no
Releases page (`/releases` redirects to Settings → Updates). With "Send changes to my
machines when I save" on (the default), Save on an edit page asks for the passkey once and
that tap both saves and sends: the page posts the form, then builds the update and has the
passkey sign it. Without the setting, or without JavaScript, Save only saves; a "Send"
button (on the saved page, in Settings and on Home when changes are waiting) runs the same
flow. Underneath nothing weakens: the update is the signed release of 0019, a WebAuthn
assertion over SHA-256 of the manifest bytes, checked with `atlas-verify` before it is
stored; numbering, storage and the client API are unchanged. An update pins only what
atlasd uses, Knowledge and public skills, each at its upstream's commit (what a machine can
fetch); project repos are out of the manifest, and a source with no upstream is left out
with a plain reason. Push rules: sending pushes Knowledge first when it has unpushed
commits; Atlas never pushes public skills (they reach machines after the owner pushes them
by hand, 0016); the "Also push project repos after saving" setting applies only to project
repos. If nothing changed since the latest update, sending asks for no tap.

## 0025 · 2026-11-27 · Installing atlasd over SSH from the hub
The owner can pair a machine without sitting at it: on the Machines page ("Or install it
from here over SSH", an address such as `you@laptop`) or with `atlas-hub add-machine
you@laptop`. The hub issues a fresh pairing code and runs the hub account's own `ssh`:
`ssh -o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new --
<target> "curl -fsSL <hub>/install | sh -s -- <code>"`, the same command the page shows
for running by hand, with stdin closed, a 3-minute limit and the output kept (the end of
it, with the code masked). Signing in uses whatever that account already has: Tailscale
SSH on the target (`sudo tailscale set --ssh`) or a key the owner added. Atlas holds no
SSH keys of its own. The address is checked strictly (`user@host` or `host`, letters,
digits, `.`, `-`, `_`; no leading `-`) and passed as one argument after `--`, never
through a local shell. `accept-new` trusts a machine's host key the first time: the
connection runs over Tailscale's WireGuard, already authenticated to the tailnet, and a
key that later changes is still refused. Trade-off: the hub's web process can now start
SSH sessions as its user, so anyone holding a passkey session can run the installer on
any machine that account can reach; it needs the passkey session and CSRF token like
every other change. `atlas-hub pair-code` prints the command for copying by hand.

The hub's systemd unit drops `PrivateTmp`, `ProtectSystem` and `ProtectKernelTunables`.
In a user service they imply a user namespace in which root's files appear owned by
`nobody`, and `ssh` then refuses the drop-ins in `/etc/ssh/ssh_config.d/` ("Bad owner or
permissions"), which most distributions include. Hiding that folder with a mount instead
would stop the hub starting where it doesn't exist. Little is lost: the hub runs as its
own user, who can't write `/usr`, `/etc` or `/proc/sys`, and home must stay writable.
The seccomp filter, `NoNewPrivileges`, `MemoryDenyWriteExecute` and the address-family
limit stay.

## 0026 · 2026-11-27 · Atlas writes Grok Build's hooks to its own file
atlasd keeps Atlas's Grok Build hooks in `~/.grok/hooks/atlas.json`, a file it owns beside
the owner's files in that folder, in Claude Code's shape (Grok reads every `*.json` there).
Grok also runs the hooks in `~/.claude/settings.json`, so when Claude Code is set up for the
login the file holds only hooks Claude's copy doesn't carry (those limited to `grok`), and
otherwise every hook meant for Grok. When Claude Code appears later the duplicates leave
`atlas.json`, and the file is removed once it holds nothing. Replaces 0022's "Atlas writes
no hooks for Grok".

## 0027 · 2026-11-27 · Where each skill goes, and bringing things into Atlas
Atlas reaches parity with the owner's T3 Code Skills page (skills series parts 1–3),
adapted to the hub and many machines. See `documents/skills.md`.
- **Placements** live in the knowledge repo as `atlas/placements.toml`, listing only
  exceptions: agents a skill is off for, and `only-projects` for a global skill used in some
  projects only. In git so they reach every machine in the signed update; not inside skill
  folders, so `npx skills` updates and the public repo stay untouched.
- **Folders per skill** come from one pure solver (`atlas_harness::placement::solve`): a
  folder no unwanted agent reads, else switch the unwanted reader off by its own setting
  (`atlas_harness::switches`: Claude Code, Codex, OpenCode, Pi), else report that the agent
  can't be excluded. With nothing off it gives the links atlasd made before.
- **Projects reach machines**: an update names each project and its remote, unpinned;
  atlasd finds the project's checkouts under `project_roots` and makes repo-local links
  (kept out of git by `.git/info/exclude`) and switches there.
- **Import moves** (owner's answer): the hub copies the item into the store and commits; on
  each machine atlasd swaps a real folder or file for Atlas's link only when it is an exact
  copy of what Atlas holds, and reports anything else. Tool-installed, built-in and plugin
  skills are listed but never imported.
- **Memory links** (owner's answer): Claude Code's per-repo memory folders become links into
  `<knowledge>/memory/projects/<project>/` after their notes are imported, so new notes go
  straight to git (0008).
- **One way to change several files**: `atlas-web::change` (session, CSRF, git lock, `main`
  for projects, clean paths, one commit per repo, restore on failure) over
  `atlas_fs::library` (bounded, link-refusing folder writes) and `Repo::commit_paths`.
- **Not now**: importing items seen only on other machines (they need Local Atlas or a
  reviewed upload) and MCP tools that let agents change skills (writes need the passkey).
- **After review** (Gemini 4 Neon, of the foundation): the solver picks the best set of
  folders (fewest agents left seeing a skill they shouldn't, then fewest switched off),
  keeping the old links whenever they are already best; an "exact copy" excludes any
  folder holding a link, a `.git` or a special file, and compares execute bits; Codex
  switches project skills off from its home `config.toml` (keyed by path), and OpenCode's
  repo `opencode.json` is never written (it is usually tracked); bulk delete and move are
  one change; public skills are committed and never pushed (0016), not refused.
- **Switch identity** (Gemini review of the integrated branch): a switch target is a
  skill's name plus the `SKILL.md` path its agent loads (`switches::SwitchOff`), not a name,
  because Codex's one home file holds every project's switches; Codex tells Atlas's tables
  apart by that exact path. `render_switches` lost its unused `atlas_before` and
  `skill_dir` arguments. OpenCode's `opencode.jsonc` is edited before `opencode.json`,
  matching OpenCode's own precedence.
- **Bringing things in, details** (import work): Claude Code's notes are copied byte for
  byte (atlasd's memory link compares bytes); host and project are recorded beside them in
  `memory/projects/<project>/index.md` and the folder name, never added to the notes. An
  imported hook's `hook.toml` gains `replaces` (the hand-made command), and the Claude Code
  and Codex formats drop the identical hand-made handler where they write the hook. Hooks
  for Grok Build and Muse, commands that look like they hold a secret, links into tools'
  folders and built-in or plugin skills are not imported. See `documents/skills.md`.

## 0028 · 2026-11-27 · atlasd links Claude Code's subagents
atlasd links each `<knowledge>/agents/<name>.md` in an update's pinned knowledge copy into
`~/.claude/agents/<name>.md` when Claude Code is set up for the login (`~/.claude` exists).
Only regular top-level `*.md` files whose name is lowercase-with-hyphens count; hidden
files are skipped, links and other names are reported. The links follow the skill-link
rules of 0020 and 0023: only links into roots Atlas manages are created, repointed or
removed (so a link into an `adopt` root is taken over and one for a subagent no longer in
the update is pruned), and an own file or a link elsewhere is reported, never replaced.
Other harnesses get no subagents until their formats are checked (`HARNESSES.md`). Reports
carry `agents_from_atlas` per harness (absent from older clients, read as 0). The update
manifest needs no change: it already pins the whole knowledge tree. This replaces the
subagent part of `link-agents`, which must stop linking `~/.claude/agents` on machines
running atlasd.

## 0029 · 2026-11-27 · Instruction files: who reads what, Use Global instead
Atlas matches the owner's T3 Code instructions work (skills series part 3). One table in
`atlas_harness::readers` says which project and home files each agent reads and how it
picks among them; the Instructions page applies it to the hub's machine. "Use Global
instead" appends an agent's own text to Global; atlasd replaces that agent's file with the
link or import only when every non-blank line is already in Global, re-checked at apply
time, so nothing is lost (0027's exact-copy rule for files). Text moved between files is
appended under an HTML comment `<!-- atlas: from <where> -->` (invisible when rendered,
says where it came from) and skipped when the target already holds every line. Claude
Code's AGENTS.md option (`pluginConfigs["cc-plugin-agents-md@builtin"].options.instructionFiles`)
is the owner's choice in `atlas/placements.toml [instructions]`, written by atlasd only
when set. A new `CLAUDE.local.md` is kept out of git through Atlas's `.git/info/exclude`
block, the only write Atlas makes inside `.git`.

## 0030 · 2026-11-27 · Checking skills from GitHub for updates
The hub compares skills installed with the `skills` CLI (`npx skills`) against their GitHub
source only when the owner presses **Check for updates**, never on a timer. It reads
GitHub's public trees and blobs API without signing in, using `reqwest` with rustls (atlasd
already depends on it), with timeouts, size caps, redirects only to the API's own origin (scheme, host and port), and every file
checked against its git SHA; archive paths are sanitised and links refused. Keeping the
owner's edits is a three-way line merge with `diffy` (MIT/Apache, no default
dependencies); clashing files are the owner's choice. The CLI's lock is updated only when
it sits in a repo Atlas commits; a global lock outside every repo is left alone, so an
edited global skill shows as updatable again (the preview says so). One lock reader,
`atlas_fs::lock`, serves updates and the skill library.
## 0031 · 2026-11-27 · Managing machines from a workstation CLI and the Machines page
An always-on hub often cannot SSH into every machine the owner can reach from their daily
workstation, and the static `machines` list in `hub.toml` couldn't be edited without editing
config on the hub.
- **Tracked machines store**: the hub keeps editable machine names in `<state>/machines.json`
  (mode 0600, seeded once from `hub.toml`'s `machines` list). On `/machines`, signed-out
  visitors see a sign-in prompt instead of a bare page; signed-in owners can track new names,
  generate machine-tailored pairing commands with a copy button, install over hub SSH, and
  remove any non-hub machine (which also unpairs its clients). The top machine picker now
  reflects paired status from `clients.json` instead of hardcoding `(not paired)`, and the
  install script starts `atlasd.service --no-block` right after enabling the timer so new
  machines report immediately.
- **Passkey-issued CLI tokens**: Settings → CLI tokens (behind a passkey session and CSRF)
  and `atlas-hub token <name>` mint revocable bearer tokens stored only as SHA-256 hashes in
  `<state>/cli-tokens.json` (mode 0600), separated from `atlasd`'s sync tokens so an ordinary
  paired login cannot add or remove other machines. Admin routes (`/api/admin/machines`,
  `/api/admin/machines/{target}`, `/api/admin/pair-code`) require both `Tailscale-User-Login`
  and a valid CLI token.
- **Workstation CLI in `atlasd`**: `atlasd auth <hub> <token>` stores the token in
  `~/.config/atlas/cli.toml` (mode 0600); `atlasd machines [list]`, `atlasd machines add
  <you@host>`, `atlasd machines track <name>`, `atlasd machines remove <name-or-id>`, and
  `atlasd pair-code` manage machines remotely. `atlasd machines add` requests a fresh
  one-time pairing code from the hub and runs the workstation's own interactive `ssh`
  (inheriting stdio so its SSH keys, agent, and prompts work natively).
- **Banner alignment**: `<main>` wraps its children in `.page-inner` (`max-width: 1180px;
  margin-inline: auto`) and `.marginalia`, `.flash`, `.empty`, and `.frontispiece` use
  `margin-block` instead of `margin` shorthands so banners align cleanly with the masthead on
  wide screens.

## 00XX · 2026-11-27 · atlasd on each machine: projects, memory links, update format
- **Update format stays 1.** `Manifest::projects` (name and remote) is added last and left
  out when empty, so a manifest without projects keeps its exact signed bytes, and older
  clients, whose manifest type doesn't refuse unknown fields, ignore it and simply link no
  project. A format bump would have made every older client refuse every update.
- **Project remotes go out without credentials** (`release::without_credentials`), and
  machines match checkouts by `release::remote_key`, read from `.git/config` and
  `.git/index` directly, so atlasd starts no git process in the owner's repos.
- **Memory links point into the owner's own knowledge checkout** (`memory_store`, else an
  adopted folder inside one, verified by its `origin`), never Atlas's local copy, which
  every pass resets to the signed commit and would delete new notes from. Without such a
  checkout no memory folder is linked and the pass says why.
- **In a checkout Atlas removes only links its exclude block lists**, and never writes a
  path git tracks; the block is add-only (shared with the hub).
- **One reader for `atlas/placements.toml` on machines**: skills and `[instructions]` come
  from one `read_placements`; only a missing file means defaults, and any other fault is
  one problem that changes no skill link, switch or Claude Code setting.
- **Codex's switches for home and project skills are rendered once per pass**; Codex's
  format holds one Atlas switch per skill name, so a second same-named skill off is
  reported. `atlas_before` isn't kept: every format marks its entries in the file.
## 0032 · 2026-11-27 · The hub is an installable web app
The hub page links a web app manifest (`/static/manifest.webmanifest`: name Atlas,
`start_url` and `scope` `/`, standalone, 192 and 512 px icons made from
`assets/logo/atlas-icon-1024.png`), and the CSP allows `manifest-src 'self'` (under
`default-src 'none'` a browser would otherwise refuse the manifest). A browser's own
"Install Atlas" then makes the launcher entry, with the Atlas name, icon and a window class
that matches it, on any desktop. Hand-made launchers (Omarchy's web app installer) still
pick up the icon through `apple-touch-icon`, but they guess the window class from the
browser, so the guide recommends the browser's install. A test checks the manifest is linked,
allowed and that each icon it lists is served at the stated size.

## 0033 · 2026-11-27 · atlasd on macOS
atlasd runs on macOS with the same code: the harness folders it writes are the same paths
there, and its hostname comes from `hostname -s` (lowercased, to match the tailnet name)
when `/proc` and `/etc/hostname` are missing. The hub serves builds named
`atlasd-<os>-<arch>` (`linux` or `macos`, `x86_64` or `aarch64`) from `<state>/dist/`, and
one install script covers both: on Darwin it checks the download with `shasum -a 256` and
loads a launchd agent (`deploy/atlasd.plist`, `~/Library/LaunchAgents/atlasd.plist`) that
runs `atlasd run` at load and every 600 s, in the login's GUI domain, or its user domain
when nobody is logged in at the Mac. macOS builds are made on a Mac, because rustls'
platform verifier links Apple's Security framework; CI-signed builds stay the target.

## 0034 · 2026-11-27 · Passkey sessions survive hub restarts
Sessions were held only in memory, and the hub is redeployed often, so each restart asked
for the passkey again. They are now saved to `<state>/sessions.json` (mode 0600), keyed by
the SHA-256 of the cookie token so the file can't be replayed as a cookie, with wall-clock
expiry. The 12-hour lifetime, cookie flags and CSRF are unchanged.

## 0035 · 2026-11-27 · Scannable skill rows without descriptions
Amends 0014 for the Skills page. Showing a truncated first line of description under every
skill row caused reader fatigue when scanning a list. Collapsed skill rows now show only
the name, any short flag (`Also global`, `Not in …`, `Broken link`), agent marks, and the
chevron; the full description (or source path for skills outside Atlas) sits inside the
inline drawer when expanded, and search still matches descriptions. Reading and attention
are treated as currency across the UI (`documents/design.md`).

## 0036 · 2026-11-27 · The Machines page polls its list; installs start a first run
The machine ledgers live in one partial (`machines_list.html`) that the page renders and
`GET /machines/list` serves alone; the page polls it every 10 seconds, and at once when
the tab comes back into view, so a newly paired machine appears without a reload. Polling,
not server-sent events: one fragment endpoint, no open connections, and a machine reports
at most every few seconds anyway. `atlas.js` skips polls while the tab is hidden (the htmx
config forbids eval, so no `hx-trigger` filter), leaves the list alone when the response is
unchanged (focus and selection survive) and reopens rows that were open, by `data-key`.
The pairing command and the SSH form sit outside the list and are never replaced. The
running head's totals ("2 machines · 1 reporting") refresh only on reload.
The install script ends with `systemctl --user start --no-block atlasd.service` so the
machine's first report reaches the hub within seconds rather than at the next timer tick.
Through systemd rather than running `atlasd run` directly: the run gets the same sandbox
as every later one, its output goes to the journal, systemd never runs it twice at once,
and `--no-block` keeps the install (and an install over SSH) from waiting for it.

## 0037 · 2026-11-27 · No "missing" flags before an update reaches this machine
Before anything was sent, every skill row said "Not in Codex and Pi" or "No agent has it"
and Instructions said "Not read by …": a flag on every row, which is no flag. Agents get
skills and Global through links atlasd makes, so until it has applied an update a missing
link means "not sent yet", which Home's "Nothing has been sent to your machines yet" line
already says. Now these flags, their Needs attention reasons and Home's "Claude can't use N
of X's skills" appear only when the hub's machine has applied everything the owner saved
(`distribute::delivered`: an update exists, a client on the machine named like the hub
reported applying the latest one, and no saved change waits to be sent). After that a
skill row flags only agents the owner wants to have it (`Placement::wants`) that lack it;
an agent switched off, or a skill used only in some projects, isn't flagged in the home
folders. The hub can't tell "not applied yet" from "applied, then changed by hand" while a
newer update is pending; it shows nothing then, as before the first send.

## 0038 · 2026-11-27 · One count for "outside Atlas"
Home counted importable outside skills (exact copies included), the Skills page's offer
counted "things" (outside skills without exact copies, plus project items), the Outside
Atlas header counted every outside row (tools' and broken links too) and the Tidy page
counted what could be brought in or compared: four numbers for one idea. Now one rule,
`import::actionable` (can be brought in or compared), drives the Outside Atlas section on
the Skills and Tidy pages; tools' and broken links and copies Atlas already has go under the
folded **Built in and others**. Home and the Skills page show the same sentence
(`tidy::offer`): "N skills on this machine aren't in Atlas", and, when the projects have
something to tidy, "; M project skills could be tidier".

## 00XX · 2026-11-28 · atlasd keeps a copy of what it replaces
atlasd may replace a file or folder whose content is all in Atlas without being the same
bytes: a machine's own `AGENTS.md` whose every line is in Global (but in another order,
with repeats or other line endings) becomes the link, Claude's `CLAUDE.md` becomes just
the import, a strict JSON settings file in its own layout is laid out again, a memory
folder whose notes differ only in an execute bit becomes the link. Before any of these,
atlasd copies the original to `~/.local/state/atlas-client/replaced/<UTC date>/<path in
the home>` (`abs/<path>` for a path outside it), in a private folder (0700). The copy is
written first, atomically, never through a link and within the `atlas_fs::library` limits;
if it can't be made, the original stays and the pass reports it. An earlier copy with
other content is never overwritten: the new one gets `.1`, `.2` … The plan decides which
actions keep a copy (`keep` on the action, so `atlasd run --dry-run` says so) and apply
copies (`atlas-client::keep`); the report sent to the hub lists each copy (`Report::kept`,
absent in older clients' reports), and atlasd prints it. Nothing is kept when the result
holds the original byte for byte: an exact skill copy adopted as a link, the import added
to a `CLAUDE.md` whose lines stay as written, a rewrite of a JSON file already in Atlas's
layout, TOML and JSONC edits made in place, links. Restoring is copying the file back.
Copies are not pruned; the owner deletes them.
