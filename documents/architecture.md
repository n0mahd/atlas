# Architecture

Hexagonal, kept lean: a pure core with ports, adapters in their own crates, and two
binaries that wire them together. Approved 2026-11-27 (decision 0007).

## Components
- **atlas-hub**: runs on whichever machine the user picks. Web UI over Tailscale, git
  working copies of the knowledge repo and of each registered project repo, the status
  store, the release builder, and later the MCP gateway and keystore.
- **atlasd**: one per user account on every machine. Pulls signed releases from the hub,
  verifies them, computes a plan, writes each harness's files, reports status.
- **Local Atlas**: a downloadable desktop app (Tauri) on every machine, paired to the
  hub. See "Local Atlas" below.

Clients store the hub's address at pairing time. No hostname appears in the code.

## Crates
| Crate | Role |
|---|---|
| `atlas-core` | Domain, ports and use cases. No I/O, no tokio, no axum. A `cargo metadata` test fails if it gains a dependency outside its allowlist (serde, thiserror, hashing). |
| `atlas-git` | Git through the `git` CLI. |
| `atlas-harness` | One pure format per harness: desired entries + current bytes → new bytes. Golden tests on real configs. |
| `atlas-fs` | Filesystem access inside a cap-std sandbox. |
| `atlas-keystore` | MCP secrets, encrypted with age. |
| `atlas-tailscale` | Identifies peers through Tailscale WhoIs. |
| `atlas-signer` | WebAuthn passkey signing of releases. |
| `atlas-web` | axum + askama + htmx UI. |
| `atlas-gateway` | The MCP gateway (phase 4). |
| `atlas-testkit` | Fakes for every port plus a contract suite each adapter must pass. |

Built so far (phase 1): `atlas-core`, `atlas-harness` (home and project layouts per
harness), `atlas-fs` (reads only: the store, each harness's folders, each project's agent
folders and Claude Code's per-repo memory) plus atomic writes for edits, `atlas-git`
(commits one file, optional push), `atlas-signer` (passkeys), `atlas-web`, and the
`atlas-hub` binary. Phase 2 editing is live (decision 0016). Phase 3: `atlas-verify`
(release signatures), `atlas-harness` formats, and `atlas-client` (`atlasd`, decision 0021).

Harnesses with a layout: Claude Code, Codex, OpenCode, Pi, Antigravity, Amp and Muse. T3
Code and Herdr are hosts that run those agents, so their layouts are only a presence marker.

`atlas-hub` and `atlasd` are the composition roots. No DI framework. Read-only screens
query the store directly instead of going through use cases.

## Contexts (modules in atlas-core)
- **Catalog**: skills, instruction files, memory, hooks.
- **Release**: numbered, signed snapshots of what goes out to machines.
- **Distribution**: targets (machine × login × harness), plans and drift.
- **Gateway**: MCP servers, tool allowlists, scoped tokens.
- **Access**: devices, logins, passkeys, audit log.
- **Shared kernel** (kept tiny): IDs, `Scope`, `HarnessKind`, `TreeHash`.

## How a change travels
1. An edit in the UI becomes a git commit.
2. The scanner flags anything risky (it never blocks silently).
3. The hub prepares release N, pinning the git tree hash.
4. The user reviews the diff and signs it with a passkey tap.
5. atlasd pulls the release and checks the signature, that N is higher than the last
   release it applied, and the tree hash.
6. atlasd computes a pure plan, viewable as a dry run.
7. atlasd writes only the entries it manages, atomically, and reports status.

Memory skips steps 3–5 by default: agents write it straight to git (decision 0008).

## Trust model
- The hub proposes; the user's passkey authorises (decision 0007). The hub cannot sign
  releases, so a compromised hub cannot push changes to every machine.
- GitHub is untrusted storage.
- atlasd runs as a user service, never as root, and never executes content it receives.
  A local stdio MCP server starts, and a hook is installed, only if it arrived in a
  signed release (decisions 0009, 0010).
- MCP keys never leave the hub. The gateway adds them to upstream calls. The UI can
  replace a key but never show it.

## Types that carry security rules
`VerifiedRelease` (only constructible by checking a signature), `Secret<T>` (no Debug,
Display or Serialize; zeroed on drop), `Plan`, `ScopedPath`, `Untrusted<Markdown>`,
`Actor`.

## Web UI (from phase 1)
- Reachable only on the hub's Tailscale address, over HTTPS with a ts.net certificate
  (passkeys require HTTPS). The hub listens on a Unix socket; `tailscale serve` fronts it
  (`deploy/`).
- Phase 1: only allowed Tailscale logins (decision 0012). Phase 2: passkey sign-in.
- Strict CSP with no inline script, CSRF tokens, `SameSite=Strict` cookies.
- Markdown is sanitised; askama auto-escapes everything else.

## Local Atlas
- Edits local files. Global edits go to the hub as proposals the user signs.
- Machine-only overrides stay on that machine and show on the hub's Machines screen.
- Project skills are edited in the local checkout and committed to `main`.
- Offline edits queue until the hub is reachable.
- Listens on localhost or a Unix socket only. Holds no MCP keys.
- WebKitGTK's passkey support is weak on Linux, so the app opens the system browser for
  the passkey tap.

## Moving the hub
Restore the git store and the keystore on the new machine, then point clients at it.
The keystore key needs an offline backup that isn't tied to one machine's hardware.
Machines without a built-in passkey sign with a phone (hybrid QR) or a hardware key.
Enrol at least two authenticators.

## Supply chain
cargo-deny, a committed `Cargo.lock`, CI actions pinned by SHA, signed release binaries.

## Threats
| Threat | Mitigation |
|---|---|
| Hub compromise | Hub cannot sign releases; keys stay in the keystore and are never shown. |
| Another tailnet device | Tailscale WhoIs plus passkey sign-in; per-login client tokens. |
| Poisoned content (skills, memory, hooks) | Scanner flags; `Untrusted<Markdown>`; sanitised rendering. |
| Secret leaks | `Secret<T>`; secrets never in git or logs. |
| Bad client writes | Pure plans, dry runs, managed entries only, atomic writes. |
| Replay of an old release | Release numbers must increase. |
| Tampering in transit or at GitHub | Signature and tree-hash checks in atlasd. |
| Supply chain | cargo-deny, locked deps, pinned actions, signed binaries. |

## Data on disk (source of truth, in git)
| Item | Location |
|---|---|
| Global skills | `<knowledge>/skills/<name>/SKILL.md`, public ones in a separate skills repo |
| Project skills | `<repo>/.agents/skills/<name>/SKILL.md` |
| Instructions | `<knowledge>/AGENTS.md`; each project's `AGENTS.md` / `CLAUDE.md` |
| Memory | `<knowledge>/memory/**.md` (frontmatter: name, description, metadata.type/host) |
| Hooks | `<knowledge>/hooks/<name>/` and `<repo>/.agents/hooks/<name>/`: a manifest (event, matcher, harnesses) plus its script |
| MCP servers | `mcp.toml` (phase 4), secrets in the hub keystore |
| Projects, machines | `atlas.toml` in the hub's data repo |

`<knowledge>` is the user's knowledge repo, configured on the hub.
