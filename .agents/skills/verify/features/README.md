# Feature map

One file per feature. Coverage is measured against this map: a run that skips a mapped
feature is incomplete, and `crates/atlas-web/tests/feature_map.rs` fails when the hub has
a route that no feature file lists.

Each file has these sections:

- **Routes**: every hub route the feature uses, one per line as `` `METHOD /path` `` with
  the path exactly as written in `crates/atlas-web/src/lib.rs` (e.g. `` `GET /skills/{home}/{name}` ``).
- **Sub-features**: what the owner can do, including error and edge states.
- **How to get to it (owner's view)**: clicks from the front page.
- **Journeys**: the spec files and test names that drive it (`e2e/journeys/...`).
- **Evidence**: the screenshots and side effects (commits, files on a machine) that prove it.
- **Gaps**: what isn't driven yet and why. Never weaken a journey to make it pass; write
  the gap here instead.
- **Gotchas**: what tripped up the last agent.
