# Memory

## Routes
- `GET /memory`
- `GET /memory/{name}`
- `GET /memory/{name}/edit`
- `POST /memory/{name}/edit`
- `POST /send/start`
- `POST /send/finish`

## Sub-features
- List with htmx filters: text (name, description), Host (Every host / Shared / each host
  named in `metadata.host`), Kind tabs (all, user, feedback, project, reference, untyped);
  host-scoped notes show their host; "No note matches."
- A project scope lists Claude Code's per-repo notes ("No memory for this project yet."),
  without a host picker.
- Note page: description, kind, host, file, "Links to" (with "(no page yet)" for missing
  targets) and "Linked from"; `[[name]]` in the text becomes a link only when that note
  exists.
- Edit and Save store notes (including `memory/hosts/<host>/…`): one Knowledge commit
  `Edit memory <name> in Atlas`, sent.

## How to get to it (owner's view)
Rail **Memory** → filters → a row → wiki links / aside links → pencil → **Save**.

## Journeys
`e2e/journeys/catalog/memory.spec.mjs`:
- the list filters by text, host and kind
- a project with no notes says so
- wiki links resolve, navigate and show backlinks
- a host's note shows its host and file
- saving a note commits it to Knowledge, and new links show
- saving a host's note commits it under that host's folder

## Evidence
Screenshots; git commit subject and path (`memory/verify-outcomes.md`,
`memory/hosts/laptop/disk.md`), text at HEAD.

## Gaps
- Per-repo agent notes (`~/.claude/projects/<repo>/memory`) are read-only and not seeded,
  so a project's notes list and its "<agent>'s notes" heading aren't driven.
- **Bring into Atlas** for agent notes has no UI yet.

## Gotchas
- `memory/index.md` has no frontmatter and isn't listed as a note.
- The Kind tabs are radio inputs inside labels: click `label.tab`.
