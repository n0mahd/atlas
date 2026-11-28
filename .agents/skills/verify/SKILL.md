---
name: verify
description: Verify Atlas end to end, every feature, and look at it. Starts a throwaway hub and client machines, drives the real UI in Chromium with a real (virtual) passkey, runs real atlasd and real harness binaries, checks security, visuals, accessibility and speed, and leaves screenshots, video and a report. Use after any change to Atlas before saying it works, when asked to verify, test end to end, QA, check visually, or "make sure everything still works", and to keep the feature map current (see Maintain).
---

# Verify Atlas

Proof that Atlas works is what the owner would see and what lands on their machines, not
green unit tests. This skill runs the product for real, in a sandbox, and makes you look
at it.

Code: `e2e/` (Playwright, Node). Feature map: `features/` next to this file. Evidence:
`target/verify/<run>/` (`target/verify/latest` points at the newest).

## What a run does

Each spec file gets its own **world** (`e2e/lib/world.mjs`), built from scratch under
`target/verify/<run>/worlds/`:

- Knowledge, public Skills and a `shop` project as git repos with local bare remotes
  (`e2e/lib/seed.mjs`), so saves commit and Send pushes exactly as with GitHub.
- A real `atlas-hub` on its own Unix socket, with `HOME`, `XDG_*` and `GIT_CONFIG_GLOBAL`
  inside the world. It never sees the real home folder, Knowledge or harness configs.
- A proxy on `http://localhost:<port>` that adds `Tailscale-User-Login`, as
  `tailscale serve` does (and strips one a client sends).
- The owner: Chromium with a CDP virtual authenticator, so first-run enrolment, sign-in
  and signing an update are the real WebAuthn ceremonies (`e2e/lib/owner.mjs`).
- Client machines (`machine(world, name)`): fake homes where the real `atlasd` pairs,
  pulls the signed update and writes every harness's files.

Every journey runs at desktop (1366×900) and phone (Pixel 7) widths, fails on any console
error or CSP violation, and records a screenshot per meaningful step, a video and a trace.

## Levels

| Level | When | Command (from `e2e/`) |
|---|---|---|
| Doctor | Before anything else; after changing `e2e/lib` | `npx playwright test journeys/doctor.spec.mjs` |
| Touched | After a change: the features it touches (find them in `features/`) | `npx playwright test journeys/<area>` |
| Full | Before merging to `main` or rolling out | `npm run verify`, then `npm run perf` (alone: timings next to other workers are noise), plus the Rust checks below |
| Nightly | Unattended, on a worker machine | See "Nightly" |

Rust checks that belong to a full run:

- `cargo test -p atlas-web --test feature_map`: every hub route is in the feature map, and
  the map lists no dead routes.
- `cargo test -p atlas-client --test model`: a bounded model check of how atlasd accepts
  updates. It covers every sequence of up to 4 answers from a possibly compromised hub, and
  checks that no unsigned, older or re-numbered update is ever accepted and no attacker key
  ever becomes trusted.

## Launch

1. `cd e2e && npm ci` (first time), then `npx playwright install chromium` if Playwright
   asks for it.
2. The first run builds `atlas-hub` and `atlasd` (`cargo build`, global setup). A build
   error stops the run. Fix it first.
3. Use at most 4 workers (`ATLAS_VERIFY_WORKERS`, default 4). Use 2 when other agents
   share the machine. See the worker limit in the global instructions.

## Doctor

The doctor spec must pass before the rest means anything: every screen returns 200 and
renders for the owner, and a request with no identity is refused. If it fails, read
`target/verify/latest/worlds/*/hub.log` first: a hub that didn't start shows why there.

## Drive

- Go through the owner's real path: click and type in the UI, and run `atlasd` and
  `atlas-hub` subcommands as the owner would. Never use internal setters, test-only
  endpoints, or hand-edits to hub state to fake progress. The only exception is the
  security specs, which play an attacker and say which one.
- Use stable handles: roles, labels, visible text, `data-*` attributes. Read the template in
  `crates/atlas-web/templates/` before writing a selector.
- Never weaken a journey to make it pass. If Atlas is wrong, keep the right assertion and
  mark the test `test.fail()` with a `bug` annotation, so it flips to an error the day it
  is fixed. Report it, and write it in the feature's Gaps if it can't be driven.

## Evidence

A run isn't verified until you have **looked** at it:

1. Read the per-step screenshots in `target/verify/latest/results/<test>/NN-<step>.png`
   for every feature the change touches, at both widths. Look for clipped or overlapping
   text, horizontal scroll on phone, broken dark mode, and anything off the design
   direction in `documents/design.md` (classic, maximalist, serif).
2. Check side effects, not just screens. The journeys assert commits in the world's repos,
   pushes to its bare remotes, and the files atlasd wrote under `worlds/*/machines/<name>/`.
   Open them when a result surprises you.
3. Visual baselines (`e2e/baselines/`) fail on unintended pixel changes. When a change is
   intended, update them with `--update-snapshots`, look at every updated image, and commit
   them with the change.
4. For a second opinion on looks, give the screenshots to a Gemini sub-agent through the
   owner's subscription (T3 `delegate_task`, provider `antigravity`, `gemini-4-neon`)
   together with `documents/design.md`. Never use OpenRouter for this.
5. Report: what ran, pass/fail per feature, bugs (with screenshot paths), visual findings,
   and the gaps that are still open. The HTML report is `target/verify/latest/report`
   (`npm run report`). To show it to the owner, serve it on this machine's Tailscale
   address and check that URL works before sharing it.

## Real harnesses

The journeys in `journeys/harnesses/` run the installed harness binaries (Claude Code,
Codex, OpenCode, Pi and the rest) against a machine's fake home. They use only offline
commands that list what each harness loaded, so the check is that the harness itself sees
the skill, obeys the switch and finds the instructions. Files that merely look right on disk
don't count. They never copy the owner's logins into a world: OAuth refresh tokens rotate,
and a copy breaks the real login. A harness without an offline probe is listed as a gap in
`features/harnesses.md`. A harness that isn't installed is skipped, with the reason given.

## Cleanup

Each world's fixture teardown stops only the processes that world started (its hub and
proxy; Playwright closes its browsers). Nothing is ever killed by process name. After an
aborted run, stop leftover hubs with `kill $(cat target/verify/<run>/worlds/*/hub.pid)`, not by name. Every file in `target/verify/<run>/` stays as
evidence. Delete old runs by hand (`rm -rf target/verify/2026-*`) when the disk needs it.

## Maintain

The feature map is how coverage is judged, so keep it true in the same commit as the
change:

- **New or changed route, screen or behaviour**: add or update its feature file (Routes,
  Sub-features, How to get to it, Journeys, Evidence, Gaps, Gotchas) and its journeys.
  `feature_map.rs` fails if a route is missing from the map.
- **Re-recorded baselines**: a baseline records whatever is on screen, broken or not, so a
  re-record is a review, not a formality. Before committing, a person or an agent opens
  every re-recorded image at desktop *and* phone width and confirms that the screen looks
  as `documents/design.md` intends. Say who looked in the commit message. Never re-record
  just to make a red run green. New screens also get a structural style check in
  `journeys/visual/` (a known control's computed style at desktop width), because an
  unclosed `@media` block can leave a screen unstyled on desktop while every other test
  passes.
- **Removed feature**: delete its journeys and its feature-file entries. The test also
  fails on routes the hub no longer serves.
- **Something tripped you up** (a flaky wait, a selector that moved, a sandbox leak): write
  it in that feature's Gotchas.
- **A gap was closed**: remove it from Gaps. A gap that stays open is debt. Add it to the
  debt register in `documents/strategy.md` if it matters.
- **Update logic changed** (`atlas-client/src/release.rs`, `pass.rs`, signing): extend the
  model in `crates/atlas-client/tests/model.rs` to cover the new rule.

## Nightly

Run the full level unattended on a worker machine: a separate checkout of the pushed
branch, with Chromium and the harness binaries installed and no access to the owner's real
home folder or logins. Browser runs are heavy, so give it the machine to itself. The
worker runs this skill at the Full level, commits nothing, and ends with the report from
Evidence step 5 (failures first). How a given owner starts such a worker is kept in their
own notes, not in this repo.
