# Accessibility

Every screen passes axe-core with no serious or critical violations in both colour schemes,
works by keyboard alone with a visible focus ring, names every control, and reflows at 200%
zoom and at 320 CSS px without sideways scrolling.

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
- `GET /signin`
- `POST /auth/signin/start`
- `POST /auth/signin/finish`
- `POST /send/start`
- `POST /send/finish`
- `GET /static/{*path}`

## Sub-features
- axe on all 12 signed-in screens plus sign-in, light and dark.
- Keyboard: Tab from the top reaches the skip link, every nav section, then the page's
  controls; Enter on the skip link lands in main; focus rings are visible.
- Keyboard: open a skill's drawer, follow "Read the skill", Edit, type, Save (which also
  sends, with the passkey tap), and the commit lands in the knowledge repo.
- Keyboard: start a passkey sign-in from a signed-out browser.
- Every visible nav link, button and `role=img` has an accessible name, rail collapsed or not.
- Settings and sign-out reachable from every page (both widths).
- Reflow at 200% (desktop) and 320px (both), and the phone top bar at 320px.

## How to get to it (owner's view)
Any page; Tab from the address bar. Skills → a row → Read the skill → Edit → Save.

## Journeys
- `e2e/journeys/a11y/axe.spec.mjs`: `axe: <screen> (<scheme>)`, `axe: sign-in (<scheme>)`.
- `e2e/journeys/a11y/keyboard.spec.mjs`: `Tab reaches the skip link, the nav and the page,
  with a visible focus ring`, `a skill is opened, edited and saved by keyboard alone`,
  `a passkey sign-in is started by keyboard alone`, `every nav item and icon button has an
  accessible name`, `Settings and sign-out can be reached from every page`.
- `e2e/journeys/a11y/reflow.spec.mjs`: `reflow at 200%`, `reflow at 320px`,
  `the top bar shows every section at 320px`.

## Evidence
Per-step screenshots (`results/<test>/NN-*.png`): each screen per scheme, the focus states
(skip link, nav link, first control in main, Save), every screen at 200% and 320px. Axe
results for each screen are attached as JSON. The keyboard save asserts the new commit
and file contents in `~/Knowledge`.

## Gaps
- No screen-reader run (NVDA/VoiceOver); axe and accessible names stand in for it.
- 200% zoom is emulated as half the CSS width, not real browser zoom. On a phone, 200% is
  ~206px, below WCAG's 320px floor, so it's skipped there.
- Known product bugs, kept as `test.fail` with a `bug` annotation:
  rail "Signed in" link named "Settings and passkeys" (2.5.3); gilt numerals and the active
  MCP nav item under 4.5:1; `ul.agent-states` with `li role=img`; no Settings or sign-out
  on phones; the top bar cuts MCP off at
  320px.

## Gotchas
- An axe test is marked as an expected failure only when every serious or critical
  violation matches a known bug (rule and element) in `KNOWN`. Anything new fails it.
  When a bug is fixed, delete its entry.
- The keyboard save uses `pdf-tools`, so the axe and visual screens of `deploy` don't change.
- The phone (Pixel 7) project still takes keyboard input; Tab order is the same.
