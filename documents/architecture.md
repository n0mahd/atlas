# Architecture

## Components
- **atlas-hub** (runs on one machine): web UI over Tailscale, git working copies
  of `~/Knowledge` and of each registered project repo, status store, and later the MCP
  gateway and keystore.
- **atlas-client** (`atlasd`, every machine, one per user account): pulls config from the
  hub, renders each harness's files through one adapter per harness, reports status.
- **atlas-core** (library): the data model (skill, instruction file, memory page, MCP
  server, machine, login, harness) and parsing of the on-disk formats. Shared by both.

## Data on disk (source of truth, in git)
| Item | Location |
|---|---|
| Global skills | `~/Knowledge/skills/<name>/SKILL.md`, public ones in `~/Skills` |
| Project skills | `<repo>/.agents/skills/<name>/SKILL.md` |
| Instructions | `~/Knowledge/AGENTS.md`; each project's `AGENTS.md` / `CLAUDE.md` |
| Memory | `~/Knowledge/memory/**.md` (frontmatter: name, description, metadata.type/host) |
| MCP servers | `mcp.toml` (phase 4), secrets in the hub keystore |
| Projects, machines | `atlas.toml` in the hub's data repo |

## Boundaries
- `atlas-core` has no network or UI code.
- Harness adapters live in the client only and depend on `atlas-core`.
- The hub talks to clients over HTTP on the tailnet; clients authenticate with a
  per-login token.

## Stack
Rust workspace. Web: axum with server-rendered HTML (askama templates) plus htmx, so the
whole project stays one language. Git via the `git` CLI (simpler, matches the user's
tooling) unless a reason appears to use a library.
