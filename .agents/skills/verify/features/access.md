# Access: passkeys, sessions, settings and CLI tokens

## Routes
- `GET /signin`
- `POST /auth/confirm/start` (a fresh tap before adding or removing a passkey or creating a CLI token; decision 0038)
- `POST /auth/confirm/finish`
- `POST /settings/signout-everywhere`
- `GET /setup/{token}`
- `POST /auth/enrol/start`
- `POST /auth/enrol/finish`
- `POST /auth/signin/start`
- `POST /auth/signin/finish`
- `POST /auth/signout`
- `GET /settings`
- `POST /settings`
- `POST /settings/keys/{id}/remove`
- `POST /settings/tokens`
- `POST /settings/tokens/{id}/remove`
- `GET /api/admin/machines`
- `GET /skills/{home}/{name}/edit`
- `POST /skills/{home}/{name}/edit`
- `GET /memory/{name}/edit`
- `GET /instructions/{scope}/{file}/edit`
- `GET /static/{*path}`

## Sub-features
- First run: with no passkey and the window open (`first_run_minutes`), the signed-in
  Tailscale login creates the first passkey; afterwards /signin is the usual sign-in.
- First run closed: the page points to `atlas-hub setup`; enrol/start is refused (403).
- Setup link (`atlas-hub setup`): enrols a passkey on another device, works once (404
  after), lands on Settings signed in.
- Sign out (rail) and sign in (passkey); a cancelled prompt shows "The passkey prompt was
  closed. Try again." and a retry works; `next` returns the owner where they were going.
- Signed out: every page reads; Settings and editors redirect to /signin; a save posted
  without a session is refused (403) and changes nothing.
- Read-only hub (no `public_url`): no sign-in control, no pencils; /signin, /settings and
  the edit routes are 404.
- Settings: passkey list (name, added, by, last used), add a passkey (another
  authenticator), remove one (not the last: no button, plus a nudge to add a second); a
  removed passkey can't sign in; the two switches (send on save, push project repos) save
  and change the editor's note and send behaviour.
- CLI tokens: create (shown once as `atlasd auth <hub> <token>`, Copy button), listed
  after reload without the secret, works on `/api/admin/machines`, revoke → 401; a blank
  name is refused with a reason.

## How to get to it (owner's view)
Rail foot: **Sign in to edit** / **Signed in** (Settings) / sign-out button. On a phone
the rail foot is hidden (see Gaps). Setup link from `atlas-hub setup` on the hub.

## Journeys
`e2e/journeys/access/access.spec.mjs` (serial, one hub with no passkey at start):
- Sign out everywhere ends every session, this one too (a second browser with the synced passkey is signed out as well)
- adding a passkey, removing one and creating a CLI token each go through the confirm tap (`/auth/confirm/*`) first; `swapAuthenticator` brings in the new device only once the enrolment starts
- first run: the owner creates the first passkey
- the rail offers Settings and Sign out, then Sign in (expected to fail on phone: bug)
- signed out, the owner reads everything but saves nothing
- a cancelled passkey prompt says so, and trying again works
- a setup link adds a passkey from another device, once
- Settings adds a passkey and removes others, never the last
- the settings switches save and change what Save does
- a CLI token is shown once, works, and stops working when revoked

`e2e/journeys/access/closed.spec.mjs`: a closed first run points to atlas-hub setup, and
its link works. `e2e/journeys/access/readonly.spec.mjs`: a read-only hub shows everything
and offers no way to change it.

## Evidence
Screenshots of each state (first run, closed, setup link, cancelled prompt, settings with
1/2/3 passkeys, token shown once / listed / revoked). The virtual authenticators'
credential counts; HTTP status of the CLI token before and after revoking; 403 bodies.

## Gaps
- Bug: on phone widths the rail foot (Sign in, Settings, Sign out) is hidden by CSS, so
  Settings and Sign out are unreachable; sign-in only via an edit pencil. The journeys
  submit the real sign-out form directly on phone to continue.
- Removing the last passkey through the UI is impossible by design (no button); the
  server's "That's your only passkey" refusal isn't driven.
- Sessions ending on hub restart (debt register) isn't driven here.
- The first-run window expiring while the page is open isn't driven (needs a 1-minute wait).

## Gotchas
- Since decision 0038 adding a passkey is two ceremonies: confirm with an enrolled passkey, then register the new one. Chrome won't choose between two virtual authenticators, so `swapAuthenticator` swaps devices at `/auth/enrol/start`.
- A credential copied into a second authenticator keeps its own signature counter; sync the counter back after using it (as a synced passkey does), or the hub refuses the older count as a clone.
- "Sign out" also matches Settings' "Sign out all": use `exact: true`.
- `enrol: false` worlds are separate workers: keep one file per world option set.
- A virtual authenticator can't enrol a second passkey for the same user (the hub
  excludes existing credentials): use another context (setup link) or
  `swapAuthenticator()` from `e2e/lib/catalog.mjs`.
- After `swapAuthenticator().restore()` the `auth` fixture's `refuse()` points at the old
  authenticator; do refusals first.
- First-run enrolment lands on /settings.
