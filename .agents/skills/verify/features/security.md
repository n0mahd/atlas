# Security

The threats in `documents/architecture.md`, the trust model, `documents/guide/security.md`
and the debt register's security notes, each driven against a real hub and a real atlasd.
These journeys play attackers, so they use raw HTTP (`request`, `rawRequest`); each test
says which attacker it plays. They assert through public interfaces only (HTTP, the UI,
atlasd, git remotes), never the hub's file layout: a compromised hub is played by
intercepting `GET /api/release` at the test proxy (`interceptRelease`), and expiry is
driven by a clock shim (`e2e/lib/clock-shim.c`, `restartHubWithClock`, `advanceClock`).
Only the secrets journey looks at files on disk, because leaked files are its subject. Tests marked `test.fail()` carry a `bug` annotation and are expected to fail
until the product changes.

## Routes
- `GET /`
- `GET /skills`
- `GET /skills/{home}/{name}`
- `GET /instructions`
- `GET /instructions/{scope}/{file}`
- `GET /memory`
- `GET /memory/{name}`
- `GET /hooks`
- `GET /mcp`
- `GET /machines`
- `GET /signin`
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
- `GET /releases`
- `POST /send/start`
- `POST /send/finish`
- `POST /machines/pair`
- `POST /machines/pair/ssh`
- `POST /machines/add`
- `POST /machines/forget`
- `POST /machines/{id}/remove`
- `GET /install`
- `GET /install/{name}`
- `POST /api/pair`
- `GET /api/release`
- `POST /api/report`
- `GET /api/admin/machines`
- `POST /api/admin/machines`
- `DELETE /api/admin/machines/{target}`
- `POST /api/admin/pair-code`
- `GET /skills/{home}/{name}/edit`
- `POST /skills/{home}/{name}/edit`
- `GET /memory/{name}/edit`
- `POST /memory/{name}/edit`
- `GET /instructions/{scope}/{file}/edit`
- `POST /instructions/{scope}/{file}/edit`
- `GET /static/{*path}`

## Sub-features
Threat table rows and the tests that prove them (all under `e2e/journeys/security/`):

| Threat / promise | Proven by |
|---|---|
| Hub compromise: the hub can't sign | `updates.spec.mjs`: edited update bytes refused; update signed by a key the machine never enrolled refused (with a control signed by the owner's key, accepted); `send.spec.mjs`: a stolen session can't finish a send without the passkey, and an old owner assertion can't sign a new update |
| Passkey enrolment | `enrolment.spec.mjs`: first run claimed once; enrol needs a session and its CSRF token once a passkey exists; setup link works once; `enrolment-clock.spec.mjs`: first-run window closes, setup link expires |
| Another tailnet device | `identity.spec.mjs` (no login, near-miss logins, socket 0600 in a 0700 folder); `stranger.spec.mjs` (login not allowed, forged header replaced by the proxy, no first-run enrolment); `sessions.spec.mjs`; `client-api.spec.mjs` |
| Hub web process starting SSH sessions | `sessions.spec.mjs`: `POST /machines/pair/ssh` refused without session, CSRF or same origin |
| Poisoned content (skills, memory, hooks) | `poisoned.spec.mjs` (sanitised rendering, CSP stops a beacon, editor shows text; scanner test); `paths.spec.mjs` (escaping hook flagged on the hub and never installed by atlasd) |
| Secret leaks | `secrets.spec.mjs`: passkey private key, session cookie, CSRF token and used pairing code appear in no file of the world; each token only in its own 0600 machine file; hub state 0600/0700 |
| Bad client writes | `updates.spec.mjs`: every refusal leaves the machine's files byte-identical; `paths.spec.mjs` |
| Replay of an old release | `updates.spec.mjs`: update 1 after update 2 refused (no rollbacks); update 1 relabelled as 3 refused |
| Tampering in transit or at GitHub | `updates.spec.mjs`: force-pushed main ignored (pinned commit used); pinned commit pruned from the remote refused; another commit stored under the pinned id refused |
| Strict CSP, CSRF tokens, SameSite=Strict cookies | `headers.spec.mjs`; `sessions.spec.mjs` |
| One-time pairing codes, 15 minutes | `pairing.spec.mjs`: single use, wrong codes, wrong hub address doesn't spend a code, expiry (with a 14-minute control) |
| Per-machine tokens, removable | `client-api.spec.mjs` (missing/malformed/unknown/cross-kind tokens refused; unpaired machine loses access); `secrets.spec.mjs` (revoked CLI token) |
| Socket open to others | `socket.spec.mjs`: the hub exits for folders 0755, 0770, 0701, 0707 |
| Links in the store (debt register) | `paths.spec.mjs`: links out of the repo and to hidden files skipped and flagged; a link to a plain file inside the repo followed |
| Path traversal | `paths.spec.mjs`: 25 encoded and plain traversals across every parameterised route read nothing; saves to traversal paths write nothing |

## How to get to it (owner's view)
- Send: Settings → Updates → **Send now** (one passkey tap), or save an edit with
  "Send changes to my machines when I save" on.
- Pair: Machines → **Pair** shows `curl … | sh -s -- <code>`; the journeys run
  `atlasd pair <hub> <code>` with that code.
- CLI token: Settings → CLI tokens → name → **Create token**; revoke with the bin icon.
- Unpair: Machines → the × on a client row.
- Problems: the front page's **Needs a look**; hooks show **Script missing**.

## Journeys
- `e2e/journeys/security/identity.spec.mjs`: no Tailscale login: every route is refused; a login that differs from an allowed one is refused; the socket is closed to other users.
- `e2e/journeys/security/stranger.spec.mjs`: a login not in allowed_logins is refused on every route; a browser can't forge the owner's login through the proxy.
- `e2e/journeys/security/sessions.spec.mjs`: cookies are HttpOnly, Secure, SameSite=Strict and host-only; every write is refused without a session, with a wrong CSRF token, or from another origin (16 routes × 8 attempts, plus a control); passkey ceremonies refuse another origin; a cross-site form in the owner's own browser can't write; the owner can find Sign out on every width; signing out ends the session: the old cookie replayed is refused.
- `e2e/journeys/security/headers.spec.mjs`: every page, signed in and signed out, carries the headers and no inline script; pages render in the browser with no CSP violation; another site can't frame the hub.
- `e2e/journeys/security/paths.spec.mjs`: traversal in any route's path is refused and reads nothing; a save to a traversal path writes nothing; links to hidden files or out of the repo are not followed, and are flagged; a memory note that links to a hidden file is flagged like a skill; a hook whose run climbs out of its folder is flagged; a machine never installs the escaping hook or the linked-out skills.
- `e2e/journeys/security/poisoned.spec.mjs`: poisoned markdown renders inert; the editor shows the poison as text, not markup; the scanner flags script, injected instructions and secret-looking tokens.
- `e2e/journeys/security/updates.spec.mjs`: the owner sends update 1 and a paired machine applies it; compromised hub: edited update bytes / key never enrolled / replay; the owner sends update 2 from an edit; compromised git host: force-pushed main, pinned commit gone, different commit under the pinned id; the owner sees each refusal on the Machines page.
- `e2e/journeys/security/enrolment.spec.mjs`: first run: the allowed login creates the first passkey; once a passkey exists, another device can't add one without a session; a signed-in owner adds a second passkey; its session's CSRF token is required; a setup link from the hub adds a passkey once.
- `e2e/journeys/security/enrolment-clock.spec.mjs`: the first-run window closes after 30 minutes; a setup link expires after 15 minutes; a fresh setup link still works after the window closed.
- `e2e/journeys/security/pairing.spec.mjs`: a pairing code works once; a wrong code gets nothing; a wrong hub address is refused without spending the code; a pairing code expires after 15 minutes.
- `e2e/journeys/security/send.spec.mjs`: closing the passkey prompt sends nothing; send needs the session, its CSRF token and the hub's origin; a stolen session without the passkey can't sign an update; the owner's passkey sends; its assertion can't be reused for another update.
- `e2e/journeys/security/secrets.spec.mjs`: full path (send, pair, run, CLI token); no secret is written where it shouldn't be; a CLI token revoked in Settings stops working.
- `e2e/journeys/security/client-api.spec.mjs`: missing, malformed and unknown tokens are refused on every API route; a token works only for its own API; pairing refuses a made-up code and a wrong hub address doesn't spend a real one; a machine the owner unpairs loses access at once.
- `e2e/journeys/security/socket.spec.mjs`: an open socket folder stops the hub from starting.

## Evidence
- Screenshots per step in `target/verify/<run>/results/journeys-security-*/NN-*.png`: Send
  from Settings, the one-time pairing command, the CLI token shown once and revoked, the
  Machines page listing each machine's refusal reason, Needs a look listing skipped links
  and the escaping hook, the poisoned pages rendered inert, a stranger refused, the
  framed hub replaced by Chromium's error page.
- Side effects asserted: atlasd's exit code and its "refused release N: …" line; a hash
  of every file in the machine's home before and after a refused run; the hub state
  folder and each repo's HEAD before and after 128 refused writes; files under the world
  root searched for each secret; file modes of the state folder and token files.
- Annotations: `atlasd said` (substitution case), `atlasd` (paths), `frame url`, and
  `bug` on expected failures.

## Paper only: green here doesn't mean covered

These protections are real, but no journey would fail if they were removed. Each line
says why, so a green run isn't read as proof of them.

- **Tailscale identity (WhoIs, `tailscale serve`)**: the test proxy stands in for
  Tailscale and always adds the identity, so removing Tailscale's own check fails nothing.
- **SSH target check and the target after `--`**: there is no real SSH host to attack; the
  fake `ssh` runs whatever it is given.
- **`Secret<T>` (no Debug/Display/Serialize, zeroed on drop)**: a type-level rule; the
  secrets journey only sees what reaches a file or log, not values held in memory.
- **Atomic writes in atlasd**: nothing injects a crash mid-write, so a non-atomic write
  would still pass every journey.
- **atlasd's own tree-hash comparison**: git's object check refuses a tampered tree
  first, so removing atlasd's comparison fails no journey.
- **Supply chain (cargo-deny, locked deps, pinned actions, signed binaries)**: outside
  what an end-to-end run can exercise.
- **Content scanner**: not built yet (architecture.md marks it planned);
  `poisoned.spec.mjs` asserts its flag as an expected failure until it exists.

## Gaps
- atlasd's own tree-hash comparison is not reached: an object stored under the pinned id
  is refused earlier by git's object hash check. Reaching it needs a SHA-1 collision.
- Session expiry (12 hours), passkey-flow expiry (5 minutes) and pending-update expiry
  (10 minutes) aren't driven yet; the clock shim could drive them.
- `POST /machines/pair/ssh` is only checked for refusal; a real SSH install needs a host.
- `/install/{name}` binaries aren't seeded in the world, so their SHA-256 check isn't
  driven (and builds aren't signed yet, per the debt register).
- MCP keys and the keystore don't exist yet.

## Gotchas
- atlasd must run asynchronously: the proxy runs in the test's own event loop, so a
  synchronous atlasd that talks to the hub deadlocks. These specs use `atlasd(m, ...)`
  from `e2e/lib/security.mjs`.
- Before restarting a stopped hub, delete its stale socket, or the wait for the socket
  returns before the new hub listens.
- atlasd writes nothing on a machine without a harness; `pairedMachine` creates
  `~/.claude` so a run has something to write.
- To make a pinned commit unreachable, amend it (rewrite history); a commit on top keeps
  it reachable and `git gc` won't prune it.
- A POST with a body that the login middleware refuses can come back as 502 from the test
  proxy (the hub answers before reading the body). Send such probes without a body.
- Playwright's `response.headers()` hides `set-cookie`; use `headerValue("set-cookie")`.
- `test.use` with worker options must be top-level, so the stranger world is its own file.
- One virtual authenticator can't enrol a second passkey for the same hub (WebAuthn's
  excludeCredentials); add a fresh authenticator, as the owner would use another device.
- The clock shim moves the wall clock and the monotonic clock. Run `atlas-hub setup`
  through `hubCliWithClock` so the link is issued on the hub's clock.
- Header values are trimmed by HTTP, so `"owner@github "` is the allowed login, not a
  near miss.
