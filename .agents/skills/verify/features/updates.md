# Skill updates

Skills installed with `npx skills` (a `skills-lock.json` entry) are checked against their
GitHub source and updated in place (documents/skills.md).

## Routes
- `GET /skills/updates`
- `POST /skills/updates/check`
- `GET /skills/{home}/{name}/update`
- `POST /skills/{home}/{name}/update`

## Sub-features
- **Check for updates**: one check at a time over every source; a page after a check asks
  GitHub nothing.
- Per skill: unedited skills update; edited ones merge; a clash is shown and nothing is
  written. The lock entry moves to the new folder hash; an execute-bit change counts.
- Project locks are written; locks outside Atlas are left alone.
- Rate limits say when to try again; redirects stay on the API's origin; a folder hash
  that doesn't add up is refused. A passkey session is needed.

## How to get to it (owner's view)
Skills → **Check for updates** → a skill with an update → **Update**.

## Journeys
Driven by HTTP tests through the router against a stand-in GitHub API with real git
(`crates/atlas-web/tests/updates.rs`).

## Evidence
Commits in the owning repo, the lock file at `HEAD`, and the stand-in's request log.

## Gaps
- No browser journey: GitHub isn't reachable from the e2e world, so the HTTP tests cover it.
