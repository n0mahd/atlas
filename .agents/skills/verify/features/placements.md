# Placements (where skills go, per-agent switches, project checkouts)

`<knowledge>/atlas/placements.toml` lists exceptions: agents a skill is off for and
`only-projects`; Send carries it and atlasd applies it, also in each project checkout
(documents/skills.md, decision 0027).

## Routes
- `GET /skills`
- `GET /skills/{home}/{name}`
- `POST /send/start`
- `POST /send/finish`
- `GET /api/release`

## Sub-features
- Per-agent switches: an agent a skill is off for gets no link, or, when it reads a folder
  that must hold the skill for others, its own setting: Claude Code `skillOverrides`,
  Codex `[[skills.config]] enabled = false`, OpenCode `permission.skill.<name> = "deny"`
  (comments kept), Pi `-skills/<name>/SKILL.md`. The owner's entries stay.
- `only-projects`: a global skill linked into the listed projects' checkouts, not home.
- Project checkouts under `project_roots` (`~/Projects`): project skills linked into
  `.claude/skills` as relative links listed in `.git/info/exclude`.

## How to get to it (owner's view)
Skills → a skill → Read the skill shows each agent's state (read only). Switches have no
UI yet; the owner edits `atlas/placements.toml` in Knowledge and commits, then Send.

## Journeys
`e2e/journeys/distribute/placements.spec.mjs`:
- the skill page has no per-agent switches yet
- switches in placements.toml reach each agent's own setting; the owner's entries stay
  (`test.fail`, bug)
- a checkout of shop gets its project skill linked, kept out of git (`test.fail`, bug)

## Evidence
- Screenshot: skill page without switches.
- Files on the machine: `~/.local/share/atlas/sources/knowledge/atlas/placements.toml`
  (the update carried it), the harness folders and switch files checked per agent.

## Gaps
- No UI to set switches or "Use in…" (`skills_place::set_agents` and `skill_ops` are stubs).
- atlasd doesn't read placements, so no switch is written (bug journey).
- Updates don't name projects and atlasd has no `project_roots` scan, so checkouts get
  nothing (bug journey). Codex/Pi project-skill switches and Claude's per-repo memory link
  can't be driven until then.

## Gotchas
- The solver usually needs no switch in the home folders (an off agent just gets no link);
  only OpenCode, which reads Claude's and Codex's folders, needs its `permission.skill`.
  The journey checks the outcome per agent (`sees`: a readable link and no switch) rather
  than which mechanism atlasd picked.
