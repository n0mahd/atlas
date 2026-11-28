# Performance

A big store (~300 skills, ~300 memory notes, 5 projects) stays quick to read, save, send
and apply.

## Routes
- `GET /`
- `GET /skills`
- `GET /skills/{home}/{name}`
- `GET /skills/{home}/{name}/edit`
- `POST /skills/{home}/{name}/edit`
- `GET /instructions`
- `GET /instructions/{scope}/{file}`
- `GET /memory`
- `GET /memory/{name}`
- `GET /hooks`
- `GET /mcp`
- `GET /machines`
- `GET /settings`
- `POST /settings`
- `POST /send/start`
- `POST /send/finish`
- `POST /api/pair`
- `GET /api/release`
- `POST /api/report`
- `GET /static/{*path}`

## Sub-features
- Time to first render (first contentful paint) and load (Navigation Timing) of every
  screen, read cold (after the 3 s store cache expires).
- Save → commit with send-on-save switched off, then Send (start → finish).
- `atlasd pair` with a code from `atlas-hub pair-code`, then one timed `atlasd run` that
  places all 300 bulk skills in `~/.claude/skills`.

## How to get to it (owner's view)
Settings → untick "Send changes to my machines when I save" → Save; Skills → a skill →
Edit → Save; front page → Send. On the machine: the install command's code, then atlasd.

## Journeys
- `e2e/journeys/perf/big-store.spec.mjs`: `a big store stays quick to read, save, send and apply`.

## Evidence
`target/verify/<run>/perf.json` (per project), compared with `e2e/perf-baseline.json`:
a number fails only when it is over 2× its baseline and over a floor (1 s for page
timings, 2 s Save, 3 s Send and atlasd run). Screenshots of the bulk skills list, the
save and the send; `atlasd-run.txt` holds the run's output.
First recorded numbers (desktop): pages 88–363 ms to load, Save 175 ms, Send 311 ms,
`atlasd run` 284 ms.

## Gaps
- The Debt register's "batched git status" item isn't visible: Save checks `git status`
  only for the paths it touches, and the UI has no bulk change that touches hundreds of
  paths. It would show only in a bulk edit, which doesn't exist yet.
- One machine and one login. Timings come from a dev build on a busy shared machine,
  hence the wide tolerance.

## Gotchas
- `machine().atlasd()` used execFileSync, which blocked this process's event loop and with
  it the world's proxy, so `pair` and `run` fail with "error sending request". Use
  `atlasdAsync` from `e2e/lib/screens.mjs`.
- Run it alone with `npm run perf` (one worker, desktop): beside other workers a 300-skill page took 1.2 s against 0.2 s alone.
- Re-record with `ATLAS_PERF_RECORD=1` and say why in the commit.
- The journey creates `~/.claude` on the machine so Claude Code counts as installed and the skills have somewhere to go.
