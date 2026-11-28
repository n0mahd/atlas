# Placements (where skills go, per-agent switches, project checkouts)

`<knowledge>/atlas/placements.toml` lists exceptions: agents a skill is off for and
`only-projects`; Send carries it and atlasd applies it, also in each project checkout
(documents/skills.md, decision 0027).

## Routes
- `GET /skills`
- `GET /skills/{home}/{name}`
- `POST /skills/{home}/{name}/agents`
- `POST /skills/{home}/{name}/use`
- `POST /send/start`
- `POST /send/finish`
- `GET /api/release`

## Sub-features
- Per-agent switches: one switch per skill on its collapsed row (all agents) and one per
  agent in its drawer and on its page. Turning two or more agents off at once asks first.
  An agent that reads the same folder as another and has no setting of its own shows a
  locked switch with the reason behind an ⓘ.
- atlasd: an agent a skill is off for gets no link, or, when it reads a folder that must
  hold the skill for others, its own setting: Claude Code `skillOverrides`, Codex
  `[[skills.config]] enabled = false` (home and project skills, keyed by path), OpenCode
  `permission.skill.<name> = "deny"` (comments kept), Pi `-skills/<name>/SKILL.md`. The
  owner's entries stay.
- "Use in…": this project only (moves the folder), globally, only these projects
  (`only-projects`: a global skill linked into the listed projects' checkouts, not home).
  A project skill limited to some projects is one change (both repos).
- Project checkouts under `project_roots` (`~/Projects`): project skills linked into
  `.claude/skills` as relative links listed in `.git/info/exclude`.
- Refusals (session ended, dirty file, locked index) show their reason in place; the form
  posts once; nothing is left changed on disk.

## How to get to it (owner's view)
Skills → open a row (or the skill's page) → the switches and "Use in…". Each change saves,
commits `atlas/placements.toml` and, with "Send changes when I save" on, asks for the tap
that sends it.

## Journeys
`e2e/journeys/distribute/placements.spec.mjs` and `e2e/journeys/harnesses/harnesses.spec.mjs`:
- switches in placements.toml reach each agent's own setting; the owner's entries stay
- a checkout of shop gets its project skill linked, kept out of git
- the real harnesses: a skill switched off for Claude Code, Codex and Pi isn't loaded by
  them and still is by the others; the shop checkout lists `stock` for every agent

## Evidence
- Screenshots: Skills with a row open (switches), a skill page with a locked agent's ⓘ open.
- Commits: `atlas/placements.toml` per switch ("Turn … off for … in Atlas").
- Files on the machine: the harness folders and switch files checked per agent.

## Gaps
- The UI switches and "Use in…" are driven by HTTP tests through the router with real git
  (`crates/atlas-web/tests/placement.rs`) and by the review-hub walkthrough; their browser
  journeys are being written on branch `feature-map`.

## Gotchas
- The solver usually needs no switch in the home folders (an off agent just gets no link);
  only OpenCode, which reads Claude's and Codex's folders, needs its `permission.skill`.
  The journey checks the outcome per agent (`sees`: a readable link and no switch) rather
  than which mechanism atlasd picked.
- The all-agents switch posts plainly when it turns everything off, because it may ask
  first on a page of its own.
