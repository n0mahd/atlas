# Visual regression

Every screen looks as approved (documents/design.md: classic, maximalist, serif) at desktop
and phone width, in the day edition (light) and the night edition (dark).

## Routes
- `GET /`
- `GET /skills`
- `GET /skills/{home}/{name}`
- `GET /skills/{home}/{name}/edit`
- `GET /instructions`
- `GET /instructions/{scope}/{file}`
- `GET /memory`
- `GET /memory/{name}`
- `GET /hooks`
- `GET /mcp`
- `GET /machines`
- `POST /machines/pair`
- `GET /settings`
- `GET /signin`
- `GET /setup/{token}`
- `GET /static/{*path}`

## Sub-features
- Each screen in light and dark: front, skills list, skills with a drawer open, skill
  detail, instructions list and detail, memory list and detail, hooks, MCP, machines,
  machines with a pairing command, settings, the edit page.
- Sign-in in each mode: passkey sign-in, setup link (`atlas-hub setup`), first run (no
  passkey, window open), closed (no passkey, window over).
- The rail collapsed on desktop, and remembered on the next page.

## How to get to it (owner's view)
The rail (desktop) or top bar (phone) for each section; Pair on Machines; the rail toggle
at the top of the rail; `/signin` signed out; the setup link the hub prints.

## Journeys
- `e2e/journeys/visual/screens.spec.mjs`: `<screen> (<scheme>)`, `skills with a drawer open`,
  `machines with a pairing command`, `rail collapsed` (desktop only), `sign-in: passkey`,
  `sign-in: setup link`, `sign-in: first run`, `sign-in: closed`.

- `e2e/journeys/visual/structure.spec.mjs`: `the stylesheet has no @media nested in @media`,
  `desktop styles apply: <screen>` (computed styles of known controls at 1280px; a
  control missing from a screen is skipped, except the always-present ones).

## Evidence
- Pixels: `toHaveScreenshot` with `maxDiffPixelRatio: 0` (playwright.config.mjs). It used to
  allow 1%, and a changed Settings sentence ("within 10 minutes" → "within a minute", 121
  pixels) passed unnoticed; exact comparison was stable over repeated runs with 4 workers.
- Words: each screen's text in light (`<screen>.txt`, port and dates normalised, masked parts
  as `[masked]`), so any wording change fails with the exact line in the diff.
Baselines in `e2e/baselines/{desktop,phone}/journeys/visual/screens.spec.mjs/*.png`
(76 images). A failing comparison attaches expected, actual and diff to the report.

## Gaps
- Real devices and Safari/Firefox rendering: Chromium only.
- Hover states aren't captured.
- Full-page screenshots on phone show the fixed film-grain overlay covering only the first
  viewport height (the rest of the page looks a shade lighter). That's how fixed elements
  appear in a full-page capture, not something the owner sees.

## Gotchas
- Don't use `toHaveScreenshot({ stylePath })`: Playwright injects it as an inline
  `<style>`, the hub's CSP blocks it, and the owner fixture fails on the CSP violation.
  `animations: "disabled"` in the config already stops transitions, and masks work.
- Masked per run: dates on Settings (`.row-desc.mono`), the pairing command (port + code),
  the first-run minutes note. Paths are shown `~/`-relative, so they don't need a mask.
- The spec uses `worldOptions: { purpose: "visual" }` so it gets its own world: a Save or
  Send from another spec on the same worker would change the front page and Settings.
- `sign-in: passkey` must request `owner`, or the world has no passkey and shows first run.
- Update baselines with `--update-snapshots` only after an intended design change, and
  look at every changed image before committing.
