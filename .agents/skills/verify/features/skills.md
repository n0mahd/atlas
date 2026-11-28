# Skills

## Routes
- `GET /`
- `GET /skills`
- `GET /skills/{home}/{name}`
- `GET /skills/{home}/{name}/edit`
- `POST /skills/{home}/{name}/edit`
- `GET /skills/new`
- `POST /skills/new`
- `GET /skills/confirm`
- `POST /skills/bulk`
- `GET /skills/{home}/{name}/files/{*path}`
- `POST /send/start`
- `POST /send/finish`
- `GET /static/{*path}`

## Sub-features
- List: Global section (private `~/Knowledge/skills` and public `~/Skills` together), an
  "Outside Atlas" section for skills agents have that Atlas doesn't, row drawer with the
  file path, file count, per-agent states and "Read the skill".
- Filter box (htmx, matches name and description; "No skill here matches.").
- **Showing** scope picker: Global or a project; a project adds "This project" first and
  nav links carry `?scope=`. Unknown project names fall back to Global.
- Machine picker: the hub is selected; other machines are listed disabled "(not paired)".
- Skill page: description, rendered body, "Lives in" (Global private / Global public /
  The <project> repo), file path, extra files, agents. 404 "Not in the atlas." otherwise.
- Edit and Save (passkey session): one commit of exactly that file, `Edit skill <name> in
  Atlas`, in the repo that owns it. Knowledge saves also send (Settings default), which
  pushes Knowledge and signs an update with one passkey tap. Public saves are never pushed.
  Project saves commit to `main` and push only if the setting is on.
- Save refusals: the file has uncommitted hand edits; the file changed since the editor
  opened (text kept in the editor); saving identical text commits nothing.
- Markdown sanitising: `<script>`, `on*=` handlers and `javascript:` links render inert.
- Library (decision 0027): **New skill** (name, one-line description, Global or this
  project; a taken or invalid name is refused in place), **Delete…** with a confirmation,
  **Select** with a bulk bar (turn on or off for one or every agent, move to Global or a
  project, delete), each one change through `/skills/confirm`; groups "From owner/repo"
  for skills installed with `npx skills`; a **Conflict** mark for same-named skills with
  different text; **Needs attention** with a reason per skill; a skill no machine will
  install (a link out of its own folder) is flagged "Machines skip it" (a project's skill
  "Not linked for every agent") with atlasd's reason.
- File viewer: the skill page lists its files; a text file opens read only (bounded,
  escaped); only files the catalog listed open, so `..`, links and made-up paths are 404.

## How to get to it (owner's view)
Front page → rail **Skills** (or the Skills tile) → expand a row → **Read the skill** →
pencil (**Edit <name>**) → edit → **Save**.

## Journeys
`e2e/journeys/catalog/skills.spec.mjs`:
- the list shows every global skill, and the filter narrows it
- Showing picks a project's skills; the machine picker lists unpaired machines
- each kind of skill has its own page
- saving a private skill commits it to Knowledge and sends it
- saving a public skill commits to the Skills repo and never pushes it
- saving a project skill commits to the project's main
- saving the same text commits nothing
- a save refuses a file with uncommitted hand edits
- a save refuses a file that changed since the editor opened
- markdown in a skill renders inert: no script, handler or javascript: link

## Evidence
Screenshots per state in `results/<test>/NN-*.png`. Side effects asserted with git: the
commit subject and that it touches exactly one file, the new text at `HEAD`, Knowledge's
bare remote at the new commit after send, the Skills and shop remotes untouched, an
unrelated hand edit in Knowledge left uncommitted, and no commit after a refusal.

## Gaps
- The library (new, delete, confirm, bulk, the file viewer, groups, Conflict, Needs
  attention) is driven by HTTP tests through the router with real git
  (`crates/atlas-web/tests/library.rs`) and by the review-hub walkthrough; its browser
  journeys are being written on branch `feature-map`. Switches and "Use in…" are in
  `placements.md`; Bring into Atlas and Tidy up in `import.md`; Check for updates in
  `updates.md`.
- The machine picker can't pick another machine: others are disabled until the hub reads
  them (needs Local Atlas).
- Per-agent marks are only driven with no agents installed on the hub ("No agent has it");
  the instructions journey covers agents present.
- Saving on a project repo that isn't on `main` (refused) is covered by unit tests only.

## Gotchas
- One world per worker, shared by every spec file on it: assert membership, not totals
  that other files may change, and undo hand edits (`git checkout`).
- The hub caches its store read for 3 s; after a hand edit wait `SNAPSHOT_TTL_MS` before
  loading a page. The editor itself reads the file fresh.
- A refused save answers 409, which Chrome logs as a console error: use
  `allowConsole(owner, /409/)`. With the page script it answers JSON `{error}` and the
  reason shows in place (posted once); a plain form post gets the HTML page.
- The bulk bar's buttons are named `action`, which shadows `form.action`; the page script
  posts to `getAttribute("action")`.
- Skill pages have two `<h1>`s (masthead and the body's `# name`): use `header.masthead h1`.
