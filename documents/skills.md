# Skills, instructions and bringing things into Atlas

How Atlas decides which agent gets each skill, and how things made outside Atlas move into
it. Decision 0027 records the choices; this page is the working description. It matches
what the owner built for T3 Code's Skills page (skills series parts 1–3), adapted to Atlas's
hub, signed updates and many machines.

## The model in one paragraph

Every skill has exactly one real folder: `<knowledge>/skills/<name>` (private),
`<public-skills>/<name>` (public) or `<repo>/.agents/skills/<name>` (a project's). Agents
reach it through links that atlasd makes in the folders each agent reads, on every machine.
Which agents get it, and whether a global skill is used everywhere or only in some
projects, is recorded in `<knowledge>/atlas/placements.toml`, which lists only exceptions. The
hub writes that file and commits it; Send carries it to the machines in a signed update;
atlasd applies it. Nothing is ever replaced that Atlas didn't make, except an exact copy of
something Atlas now holds (see "Bringing things in").

## Where each skill goes (`atlas/placements.toml`)

```toml
[global.pdf-tools]
off = ["codex", "pi"]          # agent ids: claude-code, codex, open-code, pi, antigravity, amp, muse, grok
only-projects = ["atlas"]      # used in these projects only, not in the home folders

[project.atlas.release-notes] # a skill in atlas's own .agents/skills
off = ["claude-code"]

[keep]                         # outside items the owner chose to leave (Tidy up)
items = ["skill:~/.claude/skills/omarchy"]
```

Types: `atlas_core::catalog::{Placements, Placement, Keep}`. Reading and writing:
`atlas_fs::library::{read_placements, write_placements}` (refuses to overwrite a change
made since it was read).

### Use in…

- **Globally** (default): links in the home folders, so every project sees it too.
- **This project only**: the skill's folder moves to the project's `.agents/skills`.
- **Only these projects**: one global copy, `only-projects = [...]`; atlasd links it into
  each listed project's checkout on each machine instead of the home folders.

### Per-agent switches

`atlas_harness::placement::solve` picks the folders for one skill: for each wanted agent,
the first folder it reads that no unwanted agent reads; else one whose unwanted readers can
switch the skill off by their own setting; else its first folder, reporting the unwanted
readers as `stuck_on`. With nothing off it picks exactly what atlasd picked before.

Agents with a setting (`atlas_harness::switches`): Claude Code `skillOverrides`
(`~/.claude/settings.json`; in a repo `.claude/settings.local.json`, kept out of git),
Codex `[[skills.config]]` in `~/.codex/config.toml`, OpenCode `permission.skill` in
`opencode.json(c)` (comments kept), Pi its settings file. Atlas writes only entries it
manages and keeps the owner's. Others have no switch; the UI says so and why.

## Projects on each machine

An update names each registered project and its git remote (not pinned: project content is
the project's, decision 0003). atlasd finds that project's checkouts on its machine under
`project_roots` in `client.toml` (default `["~/Projects"]`) by matching the remote, and in
each checkout:
- links project skills into the folders agents read that don't read `.agents/skills`
  (Claude Code's `.claude/skills`), as relative links, each listed in `.git/info/exclude`;
- links `only-projects` global skills the same way (pointing at its local copy of the store);
- writes per-agent switches in the repo's own files where an agent has them;
- links Claude Code's per-repo memory folder into the store (below).

## Bringing things in (one click each)

The Skills, Memory and Hooks screens each have an **Outside Atlas** part with **Bring into
Atlas** per row, **Bring all in** and **Leave it outside**, and the Skills page has **Tidy
up** (`/skills/tidy`), offered by a note at the top of the page and by Home's notice when
there is something to do. The owner's rule: after an import the original is gone and agents
read Atlas's copy.

- **What is outside** comes from `atlas_fs::outside::scan`, run with every snapshot: the
  login's harness skill folders, the built-in and plugin skills, the hooks the harness scan
  found and Claude Code's per-repo memory. Its rules are pure functions of paths
  (`place_of`, `hook_owner`), so they are tested without a disk.
- **Requests never carry paths.** A row is named by its key (`skill:~/.claude/skills/x`,
  `memory:<folder>/<file>`, `hook:<config>#<fingerprint>`; the same form `[keep]` stores,
  built with `Keep::key`, never an absolute home path). Each action re-scans and acts only on
  an item it finds again, through one `atlas-web::change` (session, CSRF, git lock, clean
  paths, one commit per repo). Code: `atlas-web/src/import.rs` and `tidy.rs`.
- On every machine, atlasd replaces a real folder or file at a managed name with Atlas's
  link **only if it is an exact copy** of what Atlas now holds (`library::Tree::exact_digest`
  for folders, which excludes any folder holding a link, a `.git` or a special file).
  Anything that differs stays and is reported, so nothing is lost.

### Skills

| Entry in an agent's skills folder | Shown as | Imported? |
|---|---|---|
| A real folder | Its own folder in `~/.claude/skills` | Yes |
| A link to a folder in the home, outside Atlas | Linked from `~/...` | Yes: the resolved folder is copied |
| A link into `/usr`, `/opt`, `/nix`, `/snap`, flatpak, a tool's own dot-folder (`~/.cua-driver`, `~/.local/share/<tool>`, `~/.cache`, `~/.t3`, `~/.config/<not an agent>`) or a package store (`node_modules`) | Installed by `<tool>` | Never |
| A link whose target is gone | Broken link | No; atlasd offers removal |
| A link into Atlas's store | (not outside) | — |
| Codex's `~/.codex/skills/.system/<name>`; each Claude Code plugin's `<installPath>/skills/<name>` from `~/.claude/plugins/installed_plugins.json` (user and managed installs, and project installs of registered projects), named `<plugin>:<name>` | **Built in and plugins**, folded | Never |

The agents' own folders (`.claude`, `.codex`, `.agents`, `.pi`, `.gemini`, `.grok`,
`.cursor`, and `.config/opencode|amp|muse|agents`) come from the harness layouts; T3 Code's
and Herdr's folders count as tools'. Built-in paths follow T3 Code's `ProvidedSkills`.

A skill goes to Global (private) by default, or to the `.agents/skills` of the registered
project it was found in. A copy never follows links inside the folder; the row says what it
leaves out, and since atlasd won't replace such an original, the owner deletes it by hand.
If Atlas has a skill of that name: an exact copy means nothing to copy (atlasd adopts it
with the next update); otherwise **Compare** shows both, and the owner keeps Atlas's
(nothing changes) or takes this machine's (Atlas's folder is replaced, one commit).

### Memory

Claude Code's `~/.claude/projects/<repo>/memory/*.md` for each registered project are
copied **byte for byte** (`MEMORY.md` included) to `<knowledge>/memory/projects/<project>/`,
because atlasd links the per-repo folder to that store folder only when every note there
is in the store unchanged; then new notes go straight to git (decision 0008). The host and
project are therefore not written into the notes: the project is the folder name, and
`memory/projects/<project>/index.md` (a map, which the catalog skips) records which machine
each note came from (`- [note.md](note.md) · from <hub>`). A Claude Code folder with its own
`index.md` keeps it, and Atlas records nothing. A note the store already has byte for byte
is skipped; one whose store copy differs needs **Use this copy**. Notes with no or invalid
frontmatter come in unchanged too; the Memory page names them by file. Memory folders for
repos Atlas doesn't know are listed by their raw folder name, not imported, until the repo is
registered.

### Hooks

A hand-made hook becomes `hooks/<name>/hook.toml` (decision 0011) plus `run.sh`
(`#!/bin/sh` and the command, executable): the event in Atlas's words, matcher and timeout
carried, `harnesses` limited to its agent, and `replaces` naming the original command. The
page shows the command with secrets masked first, and refuses a command that looks like it
holds a secret (it would go in git). Where atlasd writes the hook (Claude Code, Codex), the
format drops a hand-made handler with exactly that command, event, matcher and timeout
(`atlas_harness::hooks`, `ManagedHook::replaces`), so it doesn't run twice. Not offered:
Atlas's own `# atlas:` entries, tools' entries (Herdr's `herdr-agent-*` scripts, scripts in a
tool's folder: "Added by <tool>"), events Atlas has no name for (Notification, ...), and
Grok Build's and Muse's hooks (Atlas keeps Grok's in a file of its own, so the hand-made
one would still run; it doesn't write Muse's yet).

### Tidy up

Adapted from T3 Code's: bring outside skills in or leave them outside; drop a project's copy
of a skill that is also Global (with **Compare**); move real folders out of a project's
`.claude/skills` into `.agents/skills` (refused if the folder holds links or a `.git`); turn
a project skill back on for agents it was switched off for. Nothing changes until **Tidy up**
is pressed; then Knowledge commits first, then each project, one commit each. A box whose
item has gone since the page was opened refuses the whole run.

### Leave it outside

Records the item's key in `[keep]` of `atlas/placements.toml`, so it isn't offered again.

## The skill library

New skill (name, one-line description, this project or Global), delete (folder and links,
after confirming), move between homes, group by source (**From owner/repo** for skills
installed with `npx skills`), select with bulk on/off, move and delete, a **Conflict** mark
for same-named skills with different text, a **Needs attention** filter with a plain reason,
the folder's file tree with a read-only viewer, and **Check for updates** for skills
installed from GitHub (preview, keep-your-edits merge, the `skills` CLI's lock updated).

## What is not here yet

- Bringing in items found only on other machines: atlasd reports them; the content has to
  reach the hub through Local Atlas or a reviewed upload (debt).
- MCP tools for agents to manage skills: writes need the owner's passkey, so agent-driven
  changes need their own decision first.
- MCP servers themselves (phase 4).
