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
projects, is recorded in `<knowledge>/atlas/skills.toml`, which lists only exceptions. The
hub writes that file and commits it; Send carries it to the machines in a signed update;
atlasd applies it. Nothing is ever replaced that Atlas didn't make, except an exact copy of
something Atlas now holds (see "Bringing things in").

## Where each skill goes (`atlas/skills.toml`)

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

The Skills, Instructions, Memory and Hooks screens each have an **Outside Atlas** part with
**Bring into Atlas** per row and **Bring all in**, and the Skills page has **Tidy up**. The
owner's rule: after an import the original is gone and agents read Atlas's copy.

- The hub copies the item into the store and commits (one change, `atlas-web::change`).
- On every machine, atlasd replaces a real folder or file at a managed name with Atlas's
  link **only if it is an exact copy** of what Atlas now holds (`library::Tree::digest`
  for folders; for instruction files, every line of the file is already in the target).
  Anything that differs stays and is reported, so nothing is lost.
- **Skills**: real folders and links to folders outside Atlas. Links into folders a tool
  installed (`/usr/share/...`, a tool's own home such as `~/.cua-driver`) and agents' own
  built-in and plugin skills are listed under **Built in and plugins** and never imported.
- **Instructions**: an agent's own home file offers **Use Global instead** (its text is
  added to Global first); a project's `CLAUDE.md` can move or merge into `AGENTS.md`.
- **Memory**: Claude Code's `~/.claude/projects/<repo>/memory/*.md` become
  `<knowledge>/memory/projects/<project>/<note>.md` with `metadata.host` and
  `metadata.project`; then atlasd makes the per-repo folder a link to that store folder, so
  new notes go straight to git (decision 0008).
- **Hooks**: a hook in a harness's config becomes `hooks/<name>/hook.toml` plus a script;
  after the signed update atlasd's own `# atlas:<name>` entry replaces the hand-made one.
- **Keep**: "Leave it outside" records the item in `[keep]`, so it isn't offered again.

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
