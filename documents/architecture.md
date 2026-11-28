# Architecture

The target is hexagonal, kept lean: a pure core with ports, adapters in their own crates,
and two binaries that wire them together (decision 0007). The code is part of the way
there. This page marks what is **built** and what is **planned**; a planned item protects
nothing yet, so never rely on one. `crates/atlas-core/tests/architecture_map.rs` fails
when this page and the code disagree about which crates exist or which paths it names.

## Start here
| Task | Read first |
|---|---|
| A page or form in the hub | `crates/atlas-web/src/lib.rs` (routes), then the handler's module and `crates/atlas-web/templates/` |
| Saving an edit as a commit | `crates/atlas-web/src/edit.rs`, `crates/atlas-web/src/change.rs`, `crates/atlas-git/src/lib.rs` |
| Sign-in, sessions, passkeys | `crates/atlas-web/src/auth.rs`, `crates/atlas-signer/src/lib.rs` |
| Sending an update | `crates/atlas-web/src/distribute.rs`, `crates/atlas-web/src/release.rs`, `crates/atlas-core/src/release.rs` |
| Pairing and installing on a machine | `crates/atlas-web/src/distribute.rs`, `crates/atlas-web/src/install.rs`, `crates/atlas-web/src/remote.rs` |
| What atlasd does on a machine | `crates/atlas-client/src/pass.rs` (one run), `crates/atlas-client/src/plan.rs`, `crates/atlas-client/src/apply.rs` |
| How one agent's config is written | `crates/atlas-harness/src/lib.rs` and that format's module; golden files in `crates/atlas-harness/tests/` |
| Reading the store and agent folders | `crates/atlas-fs/src/catalog.rs`, `crates/atlas-fs/src/harness.rs` |

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
Built:

| Crate | Role |
|---|---|
| `atlas-core` | Domain types, wire formats and pure rules. No I/O, no tokio, no axum: `crates/atlas-core/tests/dependency_allowlist.rs` fails if it gains a dependency outside its allowlist. It defines no ports (traits) and no use cases yet. |
| `atlas-harness` | One pure format per harness: desired entries + current bytes → new bytes. No I/O. Golden tests on real configs. |
| `atlas-fs` | Reads the store, each harness's folders and each project's agent folders; writes edits atomically. Writes into a repo go through a cap-std directory handle; the read-only scans of agent folders use plain `std::fs`. |
| `atlas-git` | Git through the `git` CLI: commit, push, clone, fetch and checkout of a pinned commit. |
| `atlas-signer` | WebAuthn passkeys: enrolment, sign-in and signing an update. |
| `atlas-verify` | Checks an update's passkey signature. Used by the hub before storing and by atlasd before applying. |
| `atlas-web` | The hub's UI and API (axum + askama + htmx). It also holds the write logic (saving, sending, pairing, tokens, sessions), which the target design puts behind ports in the core. |
| `atlas-client` | `atlasd`: fetch, verify, plan, apply, report; and the `atlasd machines` workstation commands. |
| `atlas-hub` | The hub binary: configuration, the Unix socket, first run. |

Planned (none of these exists, and nothing below may assume they do):

| Crate | Role |
|---|---|
| `atlas-keystore` | MCP secrets, encrypted with age. |
| `atlas-tailscale` | Identifies peers through Tailscale WhoIs, replacing trust in the header `tailscale serve` sets. |
| `atlas-gateway` | The MCP gateway (phase 4). |
| `atlas-testkit` | Fakes for every port plus a contract suite each adapter must pass. Waits on the ports. |

Phase 2 editing is live (decision 0016). Phase 3 is the client (`atlasd`, decision 0023).

Harnesses with a layout: Claude Code, Codex, OpenCode, Pi, Antigravity, Amp, Muse and Grok
Build. T3 Code and Herdr are hosts that run those agents, so their layouts are only a
presence marker.

`atlas-hub` and `atlasd` are the composition roots. No DI framework. Read-only screens
query the store directly instead of going through use cases.

## Rules to keep
- `atlas-core` and `atlas-harness` do no I/O. The first is enforced by the allowlist
  test; the second is convention (debt register).
- Production code runs `git` only through `atlas-git`.
- Only `atlas-hub` and `atlasd` read configuration and the environment.
- New write logic goes in a function that takes no axum or `AppState` types, so it can
  move behind a port later; the handler only checks the session and CSRF token, calls
  it and renders. Do not add to the handlers that mix the two.
- Every state-changing route checks the passkey session and the CSRF token before it
  reads or changes anything.

## Contexts
In `atlas-core` today:
- **Catalog**: skills, instruction files, memory, hooks, placements.
- **Release**: numbered, signed snapshots of what goes out to machines.
- **Distribution**: what a machine reports and what pairing exchanges.
- **Shared kernel** (kept tiny): IDs, `Scope`, `HarnessKind`, `TreeHash`.

Elsewhere or planned:
- **Access** (logins, passkeys, sessions, machine and CLI tokens) lives in `atlas-web`
  and `atlas-signer`. Each session names the passkey that opened it; adding or removing
  a passkey and creating a CLI token also need a fresh tap, which yields a single-use
  grant for that action and session (decision 0038). One-time codes are checked and used
  under one lock (decision 0039). There is no audit log yet.
- **Gateway** (MCP servers, tool allowlists, scoped tokens) is planned for phase 4.

## How a change travels
1. An edit in the UI becomes a git commit.
2. Planned: a scanner flags anything risky (it never blocks silently). Today nothing
   inspects content before it is saved or sent.
3. Save sends (decision 0024): the hub pushes Knowledge if needed and builds update N
   (a signed release underneath), pinning Knowledge and public skills at their upstream
   commits and tree hashes. Project repos aren't pinned: the update only names each
   project and its remote, so atlasd can find its checkouts (decision 0027).
4. The same Save asks for one passkey tap, which signs the update; the hub checks the
   signature before storing it. The UI calls these "updates", never "releases".
5. atlasd pulls the update and checks the signature, that N is higher than the last
   one it applied, and the tree hash.
6. atlasd computes a pure plan, viewable as a dry run.
7. atlasd writes only the entries it manages, atomically, and reports status.

Memory skips steps 3–5 by default: agents write it straight to git (decision 0008).

## Trust model
- The hub proposes; the user's passkey authorises (decision 0007). Nobody can send a
  change to the machines without a tap from the owner's passkey: not a stolen session,
  not another device on the Tailscale network, not GitHub.
- The limit of that promise: the hub serves the page that asks for the tap, so a hub
  machine or account that has itself been taken over can misuse the owner's next tap.
  The passkey protects against everyone who cannot get the owner to tap; it does not
  make a compromised hub harmless. Signing on a device that shows what is being signed
  (Local Atlas) is the planned answer.
- Who can act is guarded by a tap too: once a passkey exists, adding another, removing
  one and creating a CLI token each need a fresh tap from an enrolled passkey, and an
  enrolment finishes only under the authority it started with (decision 0038).
- GitHub is untrusted storage.
- atlasd runs as a user service, never as root, and never executes content it receives.
  A local stdio MCP server starts, and a hook is installed, only if it arrived in a
  signed release (decisions 0009, 0010).
- A machine trusts the hub once, at pairing: the installer and the atlasd build come
  from the hub, checked only against the hub's own checksum, and the owner keys a
  machine will accept arrive then.
- Planned with phase 4: MCP keys never leave the hub. The gateway adds them to upstream
  calls. The UI can replace a key but never show it.

## Types that carry security rules
Built: `Untrusted<Markdown>` (`crates/atlas-core/src/untrusted.rs`; raw text is reached
only by the sanitiser and by templates that escape it) and atlasd's `Plan`
(`crates/atlas-client/src/plan.rs`; what a run will write, computed before anything is
written and shown by `atlasd run --dry-run`).

Planned: `VerifiedRelease` (only constructible by checking a signature; today
`atlas_verify::verify` returns a plain result that callers must not ignore), `Secret<T>`
(no Debug, Display or Serialize; zeroed on drop), `ScopedPath`, `Actor`.

## Web UI (from phase 1)
- Reachable only on the hub's Tailscale address, over HTTPS with a ts.net certificate
  (passkeys require HTTPS). The hub listens on a Unix socket; `tailscale serve` fronts it
  (`deploy/`).
- Reading: only Tailscale logins listed in the hub's config, taken from the
  `Tailscale-User-Login` header that `tailscale serve` sets (decision 0012). Writing: a
  passkey session as well (decision 0016).
- Strict CSP with no inline script, CSRF tokens, `SameSite=Strict` cookies.
- Markdown is sanitised; askama auto-escapes everything else.
- Pairing: the Machines page (or `atlas-hub pair-code`) issues a one-time code such as
  `K7QF-9XMR2D` and shows `curl <hub>/install | sh -s -- <code>`; the hub takes at most
  five wrong codes a minute (decision 0039). With a passkey session the hub can also run it
  on a machine itself over SSH (`atlas_web::remote`, used by the page and by
  `atlas-hub add-machine`; decision 0025).

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
Built: a committed `Cargo.lock`, a pinned toolchain (`rust-toolchain.toml`), and CI
(`.github/workflows/ci.yml`) that runs fmt, clippy and the tests on every push, with its
actions pinned by commit.
Planned: cargo-deny, and release binaries built and signed by CI (today the owner builds
them and the hub hands them out).

## Threats
| Threat | Mitigation |
|---|---|
| Hub compromise | Not solved. An update needs a tap from the owner's passkey, which stops an attacker who only reads the hub's files or holds a session; but a hub that is taken over serves the page that asks for the tap and can misuse the next one, can withhold updates, and is trusted by any machine paired afterwards. Planned: signing in Local Atlas, signed atlasd builds, and MCP keys in a keystore that never shows them. |
| Another tailnet device | Only listed Tailscale logins can read; changes need a passkey session; each machine has its own token. Planned: Tailscale WhoIs instead of the header. |
| Hub web process starting SSH sessions | Only behind a passkey session and CSRF; the target is checked strictly and passed after `--`, never through a shell (decision 0025). |
| Poisoned content (skills, memory, hooks) | `Untrusted<Markdown>` and sanitised rendering keep it from running in the hub's pages. Nothing yet checks what the text tells an agent to do. Planned: a scanner that flags it. |
| Secret leaks | Tokens are stored as hashes; secret-looking values in hook commands are masked on screen (`mask_secrets`). Planned: `Secret<T>`. Nothing yet stops a secret being typed into a file that goes to git. |
| Bad client writes | Pure plans, dry runs, managed entries only, atomic writes. |
| Replay of an old release | Release numbers must increase. |
| Tampering in transit or at GitHub | Signature and tree-hash checks in atlasd. |
| Supply chain | Locked dependencies, pinned toolchain, CI with pinned actions. Planned: cargo-deny and signed binaries. |

## Data on disk (source of truth, in git)
| Item | Location |
|---|---|
| Global skills | `<knowledge>/skills/<name>/SKILL.md`, public ones in a separate skills repo |
| Project skills | `<repo>/.agents/skills/<name>/SKILL.md` |
| Instructions | `<knowledge>/AGENTS.md`; each project's `AGENTS.md` / `CLAUDE.md` |
| Memory | `<knowledge>/memory/**.md` (frontmatter: name, description, metadata.type/host); Claude Code's per-repo notes under `memory/projects/<project>/` |
| Where each skill goes | `<knowledge>/atlas/placements.toml`: only exceptions (agents off, only-projects, items kept outside); see `documents/skills.md` |
| Hooks | `<knowledge>/hooks/<name>/` and `<repo>/.agents/hooks/<name>/`: a manifest (event, matcher, harnesses) plus its script |
| Subagents | `<knowledge>/agents/<name>.md`, linked into `~/.claude/agents` (Claude Code only, decision 0028) |
| MCP servers | `mcp.toml` (phase 4), secrets in the hub keystore |
| Projects, machines | `atlas.toml` in the hub's data repo |

`<knowledge>` is the user's knowledge repo, configured on the hub.
