# Bring into Atlas and Tidy up

Skills, Claude's memory notes and hooks that agents have outside Atlas come into Atlas in
one click each, or all at once. Afterwards the agents point at Atlas and the outside copy
is gone (decision 0030, documents/skills.md).

## Routes
- `POST /import/skill`
- `POST /import/skill/theirs`
- `POST /import/skills`
- `POST /import/note`
- `POST /import/notes`
- `POST /import/hook`
- `POST /import/hooks`
- `POST /import/keep`
- `GET /import/compare`
- `GET /import/compare-copies`
- `GET /skills/tidy`
- `POST /skills/tidy`

## Sub-features
- **Bring into Atlas** per skill, note or hook, and **Bring all in** per kind: the item is
  copied into the Knowledge repo (a skill without the links inside it; a note byte for
  byte; a hook as a valid `hook.toml` and executable script) and committed once. atlasd
  then replaces the outside copy, keeping a copy of anything not byte-identical under
  `~/.local/state/atlas-client/replaced/<date>/`.
- A clash with an identical item in Atlas copies nothing; a different one shows
  **Compare** (`/import/compare`) and can take theirs (`/import/skill/theirs`).
- **Leave outside** (`/import/keep`): recorded in placements.toml `[keep]` and not offered
  again.
- **Tidy up** (`/skills/tidy`): duplicate copies across homes and repos, compared side by
  side (`/import/compare-copies`), fixed in one change with one commit per repo.
- Refusals: a dirty store, an unknown item, no session. Nothing is changed.

## How to get to it (owner's view)
Skills → "Outside Atlas" → **Bring into Atlas** / **Bring all in** / **Compare** /
**Leave outside**; Memory and Hooks have the same section. Skills → **Tidy up**.

## Journeys
Driven by HTTP tests through the router with real git (`crates/atlas-web/tests/import.rs`):
- a skill is copied and committed without the links inside
- home skills and Tidy count outside Atlas the same way
- an identical clash copies nothing; a different clash is compared and theirs can replace
  Atlas's
- Claude's notes come in byte for byte and identical ones are skipped
- a hook becomes a valid hook.toml and script
- left outside is recorded and not offered again
- changes are refused when the store is dirty or the item is unknown
- Tidy up commits once per repo

atlasd's side (adoption, memory links, the replaced copies) is in `atlasd.md`.

## Evidence
Commits in the Knowledge repo (one per change), the files at `HEAD`, placements.toml
`[keep]`, and the outside copies left untouched by the hub.

## Gaps
- Browser journeys are being written on branch `feature-map`.

## Gotchas
- The hub never deletes the outside copy itself; atlasd replaces it on the machine after
  the update arrives.
