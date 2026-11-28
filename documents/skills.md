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
made since it was read). Pages read placements from the store snapshot
(`Catalog::placements`, and `Catalog::placements_error` when the file is invalid, which
turns the switches off); only a change reads the file again, inside its apply step. The one
way a change writes it is `atlas-web::change::rewrite_placements` (inside a larger change)
or `change::edit_placements` (the file alone, one commit).

### Use in…

- **Globally** (default): links in the home folders, so every project sees it too.
- **This project only**: the skill's folder moves to the project's `.agents/skills`.
- **Only these projects**: one global copy, `only-projects = [...]`; atlasd links it into
  each listed project's checkout on each machine instead of the home folders.

### Per-agent switches

`atlas_harness::placement::solve` picks the best set of folders for one skill: fewest
agents left seeing a skill they shouldn't (`stuck_on`), then fewest switched off by their
own setting, then fewest wanted agents seeing it twice, then fewest links. When the simple
choice (each wanted agent's first folder that no unwanted agent reads) is already best, it
is kept, so with nothing off the links are exactly what atlasd made before.

A global skill limited to some projects is solved as linked into each project, not the
home folders, so the page's reasons match where the links really go. Turning a skill off
for two or more agents at once asks first.

Agents with a setting (`atlas_harness::switches`; formats and sources in
`crates/atlas-harness/HARNESSES.md`): Claude Code `skillOverrides` (`~/.claude/settings.json`;
in a repo `.claude/settings.local.json`), Codex `[[skills.config]]` in `~/.codex/config.toml`
for home and project skills alike (keyed by the skill's path), OpenCode `permission.skill`
in the home `opencode.jsonc`, else `opencode.json` (`.jsonc` is edited when both exist and
created when neither does),
Pi `skills` in `~/.pi/agent/settings.json`. Atlas marks its own entries and never changes
the owner's. Each skill to switch off is a `SwitchOff { name, path }`, `path` being the
`SKILL.md` the agent loads: Codex keys its entries by that exact path (a project's skill
must carry one), so same-named skills in two projects, or a global one and a project's, are
switched separately; the other formats are keyed by name. Others have no switch; the UI says so and why.

## Projects on each machine

An update names each registered project and its git remote (`Manifest::projects`, not
pinned: project content is the project's, decision 0003; credentials are taken out of the
URL). atlasd looks at the direct children of `project_roots` in `client.toml` (default
`["~/Projects"]`, at most 500 entries, links not followed) and reads each one's
`.git/config` directly: a checkout whose `origin` is the project's remote, written in any
form (`git@host:o/r.git`, `ssh://`, `https://`, trailing `.git` or `/`), is that project.
It reads `.git/index` for the paths git tracks; no git process runs in the owner's repos.
In each checkout:
- **Project skills** (`.agents/skills/<name>`) get relative links (`../../.agents/skills/<name>`)
  in the folders `placement::solve` picks for the agents that want them, typically Claude
  Code's `.claude/skills`. A folder where git already tracks an entry of that name counts
  as holding the skill (a committed link the team shares), so its readers are switched off
  rather than linked.
- **`only-projects` global skills** are linked the same way, to Atlas's local copy.
- **Exact copies** at a wanted name become links, as in the home folders.
- **Switches**: Claude Code's go in the repo's `.claude/settings.local.json`; Codex's for
  project skills go in `~/.codex/config.toml`, keyed by the skill's absolute path, in the
  same single render as the home's. Codex can switch off only one skill of a name per file,
  so a second checkout with a same-named skill off is reported, not switched.
- **Git stays clean**: every link and switch file Atlas makes is added to Atlas's block in
  `.git/info/exclude` (`instructions::exclude_with`: add-only, shared with the hub's
  `/CLAUDE.local.md`) before it is made. Atlas never writes a path git tracks (reported
  instead), and in a checkout removes only links its block lists, so the owner's own links
  stay. Lines for links that are gone stay in the block; they are harmless.

### Claude Code's memory folders

For each checkout, Claude Code's `~/.claude/projects/<repo>/memory` becomes a link to
`<knowledge>/memory/projects/<project>/`: created when missing (if that folder exists),
kept or repointed when it is a link into a folder Atlas manages, and a real folder becomes
the link only when every file in it is a note (`*.md`, no subfolders or links) already
there byte for byte, checked again just before the swap. Otherwise atlasd reports "N notes
in <path> aren't in Atlas yet" and leaves it.

`<knowledge>` here is **the owner's own checkout** of the knowledge repo, never Atlas's
local copy: that copy is reset to the signed commit on every pass (`checkout --force` and
`git clean`), so notes an agent wrote through a link into it would be deleted ten minutes
later and never reach git. atlasd uses `memory_store` from `client.toml` (`atlasd pair
--memory-store ~/Knowledge`), else the first `adopt` folder inside a checkout, and only
when that checkout's `origin` is the knowledge repo's remote. With none, nothing is linked
and the pass says why. The trade-off: new notes reach git through the owner's own sync
(such as `~/Knowledge/bin/sync`), not through Atlas's signed updates, so a machine without
such a checkout keeps Claude's notes in Claude's own folder until the hub imports them.

## Bringing things in (one click each)

The Skills, Memory and Hooks screens each have an **Outside Atlas** part with **Bring into
Atlas** per row, **Bring all in** and **Leave it outside**; Instructions works through **Use
Global instead** (below). The Skills page also has **Tidy up** (`/skills/tidy`), offered by a
note at the top of the page and by Home's notice when there is something to do. The owner's
rule: after an import the original is gone and agents read Atlas's copy.

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
  Anything that differs stays and is reported, so nothing is lost. Whatever atlasd
  replaces without the new content holding it byte for byte is first copied to
  `~/.local/state/atlas-client/replaced/<UTC date>/`, named in the pass's report, and
  restored by copying it back (decision 0037).

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

### Instructions

The Instructions page shows, for each file, which agents read it directly,
through a link, through Claude's `@` import or through Claude's AGENTS.md option, and
why the others don't (`atlas_harness::readers`, sourced in `HARNESSES.md`; computed for
the hub's own machine). An agent's own home file offers **Use Global instead**: its text
is appended to Global and committed; atlasd then swaps the file for the link (Claude's
`CLAUDE.md` for just the import) on each machine only where every non-blank line of it
is in Global (`instructions::contained`, re-checked when applied), keeping a copy of the
file first (above). A project's
`CLAUDE.md` can become `AGENTS.md` or merge into it, and the same confirmation offers
Claude's option when Claude would still skip `AGENTS.md`; **Move to Global** and **Copy
to this project** move text the other ways. Moved text is appended after a blank line
and `<!-- atlas: from <where> -->`, and nothing is added that the target already holds.
A new `CLAUDE.local.md` goes in Atlas's block of `.git/info/exclude`, so it is never
committed (and git can't undo it; the confirmation says so). Claude's choice lives in
`atlas/placements.toml` as `[instructions] claude-reads-agents-md = "when-no-claude-md" |
"alongside" | "never"`; without it atlasd leaves Claude's setting alone.

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

How it works (`atlas-web::skill_ops`, `library`, `library_pages`):
- Every create, delete, move or bulk switch is one change through `atlas-web::change`: one
  commit per repo, the destination repo first; the placements file is read and written
  inside the change. Turning several skills off, moving and deleting go through a
  confirmation page that lists each folder, record and entry that changes. "Use in… →
  Only these projects" on a project's skill is one change too: the knowledge repo's commit
  holds the folder and its `only-projects` entry, so it is never global for every project.
- Every handler that changes something checks the session and CSRF token first
  (`change::authorise`), before reading the store or the form. The page script
  (`passkey.js`, `Accept: application/json`) gets every refusal as JSON `{error}`
  (`ChangeError::into_response`, `change::form_error`) and shows it in place; any other
  answer makes it post the form again as a plain one.
- A skill's `skills` CLI record moves with it (`atlas_fs::lock::convert_record`, as T3
  Code's `convertRecord`) only when both locks sit in repos Atlas commits. The global lock
  usually lives in `~/.agents`, outside git: then Atlas changes neither lock and the
  confirmation says so. A delete drops the record only from a lock Atlas commits.
- Groups, Conflict marks and Needs attention are worked out once per read of the store
  (`library::Index`): conflicts compare exact folder fingerprints of same-named copies in
  the store and in agents' own folders.
- The file viewer opens only a file the catalog listed for that skill, refuses links, shows
  at most 256 KB, and renders `.md` through the sanitiser; scripts are flagged, never run.

## Updates

**Check for updates** (Skills page, signed in; never automatic) compares each store skill
that has a `skills` CLI lock record from GitHub with its source (`atlas-web::updates`,
records from `atlas_fs::lock`). Global skills (knowledge, public) use the global lock
(`$XDG_STATE_HOME/skills/.skill-lock.json`, else `~/.agents/.skill-lock.json`), project
skills their repo's `skills-lock.json`. A check lists each source repo's tree once through
GitHub's API without signing in (at most 20 repos, four at a time, 10 s and size limits;
repos never listed go first, then the oldest listings, so every repo is reached over a few
checks). When GitHub cuts a big repo's listing off, the check lists each skill's folder on
its own too, and for an edited project skill it fetches the few files that tell whose change
it is (never `.git` or `node_modules`). The listings merge with earlier checks' and stay in
memory for the hub's run; nothing is written, and showing the Updates page asks GitHub
nothing. A folder's SHA from a listing is trusted only once its entries add up to it.
Redirects are followed only to the API's own origin. When GitHub's limit is
used up the page says when to try again and nothing more is asked until then. **Preview**
shows each changed file as a diff before anything is written. With the installed version
(the record's baseline) a file only the source changed takes theirs, one only the owner
changed keeps theirs, one both changed is merged line by line, and clashing lines are the
owner's choice per file (keep mine or take theirs). A file whose execute bit
changed counts as changed. A global record's baseline is a git tree
SHA, fetched again from GitHub; a project record holds only a hash of the files, so for an
edited project skill every difference is the owner's choice. **Update** is one change
(`change::commit_change`): the folder is rewritten (execute bits kept) and the lock record
gets what `npx skills update` would write, so the CLI then sees the skill as current. The
folder is read again under the git lock, and an update is refused if the folder changed
since the preview. A lock
that isn't inside a repo Atlas commits (the global one usually isn't) is left alone and the
preview says so. Links, submodules and unsafe names in a source are refused; fetched files
are never run; everything is checked against its git SHA and shown escaped.

## What is not here yet

- Bringing in items found only on other machines: atlasd reports them (`Report::outside`:
  name, path, own folder / link / broken, link target and exact digest, up to 300 per
  pass); the content has to reach the hub through Local Atlas or a reviewed upload (debt).
- MCP tools for agents to manage skills: writes need the owner's passkey, so agent-driven
  changes need their own decision first.
- MCP servers themselves (phase 4).
