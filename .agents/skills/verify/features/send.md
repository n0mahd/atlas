# Send (updates)

Saved changes become a numbered update that the owner's passkey signs (decisions 0019,
0024). The UI says "update", never "release".

## Routes
- `POST /send/review` (no-JS: builds the pending update and goes to its check page)
- `GET /send/review/{id}` ("Check before sending", decision 0040)
- `GET /`
- `GET /settings`
- `POST /send/start`
- `POST /send/finish`
- `GET /releases`
- `GET /memory/{name}/edit`
- `POST /memory/{name}/edit`

## Sub-features
- Send on Home ("Needs a look" row with **Send**) when changes are waiting, and **Send now**
  in Settings → Updates; one passkey tap signs update N.
- Send pushes Knowledge first when it has unpushed commits; public skills are never pushed.
- Save sends: with "Send changes to my machines when I save" on, Save on an edit page saves
  and sends with the same tap ("Saved · on your machines within 10 minutes").
- Nothing new: no tap, no new update; Settings says "Your machines have every saved change".
- Passkey prompt closed: "The passkey prompt was closed. Try again." on Home; after a save,
  "Saved. Not on your machines yet" with a **Send** button in the flash.
- Public skills with unpushed commits: "Public skills reach your machines after you push
  them." Once the owner pushes by hand, Home counts the change and Send carries it.
- Settings → Updates lists "Update N · date · passkey name", newest first.
- `/releases` redirects to Settings → Updates.

## How to get to it (owner's view)
Home → Needs a look → Send. Or Settings → Updates → Send now. Or any Knowledge edit page
(Memory → a note → Edit) → Save.

## Journeys
- `e2e/journeys/distribute/review.spec.mjs`: a hook can't be sent without its check page having been shown (no challenge from `/send/start` until the page was served); a hook pushed after the page was opened is not in what machines receive, and gets its own check next time.
- Every other Send goes through the check page when it lists hooks or passkeys: the owner fixture screenshots it ("check before sending") and confirms (`autoReview` in `e2e/lib/owner.mjs`). Journeys about the page call `manualReview(page)`.
`e2e/journeys/distribute/send.spec.mjs`:
- Send on the front page pushes Knowledge and signs the next update
- Save sends: one tap saves the note and sends it
- Send with nothing new asks for no tap and adds no update
- Closing the passkey prompt sends nothing
- Public skills reach machines only after the owner pushes them
- the old Releases page leads to Settings → Updates

Also used by every other distribute journey (`sendNow` in `e2e/lib/distribute.mjs`).

## Evidence
- Screenshots: home with changes waiting / after sending, settings lists the update,
  editing a memory note / saved and sent, passkey prompt closed, saved not sent, public
  skills note.
- Side effects: the Knowledge bare remote's `main` equals the local HEAD after Send; the
  Skills bare remote is unchanged until the owner pushes; the virtual authenticator's
  sign count goes up by exactly one per send (zero for nothing new); hub log line
  `update N signed and sent`.

## Gaps
- The note a send carries (`SignOptions.note`, e.g. the public-skills note or a source
  left out) is shown only when there is nothing else to send: `passkey.js` ignores it on a
  real send, so the owner never sees it then. Not asserted as a bug; noted.
- Send now with nothing new still lands on `?flash=sent` ("On your machines within 10
  minutes"), which reads as if something was sent. The journey asserts no update and no tap.
- Two tabs sending at once (one pending update per hub) isn't driven.

## Gotchas
- A first update lists every hook, so after a worker restarts (a failed test gives a fresh world) the next Send shows the check page again.
- Hook scripts must be committed executable (`git update-index --chmod=+x`), or atlasd skips them and the page flags them; `seed.mjs` commits `*.sh` as 0755.
- With nothing new, Send now answers "Your machines already have everything." in place, with no navigation.
- The hub reuses a read of the store for 3 s; after a commit made by hand outside Atlas,
  wait 3 s before expecting Home's "N changes" count to move (Send itself invalidates).
- `auth.refuse()` must be undone in a `finally`, or every later tap in the worker fails.
- Update numbers are shared by every test in a file (one world per worker): read the count
  from Settings first instead of assuming update 1.
