# Privacy and data inventory

What Atlas holds about people, where it goes and how long it stays. The `privacy-check` skill
keeps this file true: every merge to `main` that changes personal data, logging, storage or third
parties updates it in the same commit.

Last full check: 2026-11-27 (`privacy-check` baseline).

## Principles

- Atlas is single-owner and self-hosted: the hub runs on one of the owner's machines, reachable
  only over their Tailscale network. Nothing is sent to a service Atlas runs.
- Collect the minimum the feature needs; every new personal field is an explicit decision recorded here.
- No tracking of individuals. No usage or error telemetry.
- Secrets never go in git or logs (`Secret<T>`; MCP keys in the hub keystore).

## Personal data held

All on the hub, under its state folder (default `~/.local/state/atlas-hub`) unless noted.

| Data | Where | Why | Who can see it | Retention | Deleted by |
| --- | --- | --- | --- | --- | --- |
| Allowed Tailscale logins | hub config (`hub.toml`) | Only these logins may open the hub | Hub owner | Until edited | Editing the config |
| Passkey public keys, names, who added them | `passkeys.json` | Passkey sign-in and signing updates | Signed-in owner (Settings) | Until removed | Settings → remove key |
| Paired machines: machine name, OS user name, paired / last-seen times, last status report (harnesses found, counts, problems) | `clients.json`, `machines.json` | Machines page, sending updates | Signed-in owner | Until unpaired | Machines → remove |
| Client and CLI tokens | `clients.json`, `cli-tokens.json` | Authenticate atlasd and the CLI | Nobody: stored only as SHA-256 hashes (0600); each CLI token's last use, at most once a minute | Until removed | Unpair / remove token |
| One-time codes and recent wrong pairing attempts | `setup-links.json`, `pairing-codes.json` (0600, SHA-256 only; times of wrong codes in the last minute, a count since the last issue) | Setup links, pairing, the wrong-code limit (the pairing-codes decision (00XX)) | Nobody | Until used, expired or voided | Use, expiry, issuing a new code |
| Skills, instructions, memory, projects | The owner's git repos | The product itself | Owner; the repos' own visibility applies | Git history | Git |
| SSH targets (`user@host`) typed for an SSH install | Request only; logged (see Logs) | Install atlasd from the hub | Hub owner | Journal retention | journald |

## Logs

| Log | Contents | Personal data? | Where / readable by | Retention |
| --- | --- | --- | --- | --- |
| `atlas-hub` (tracing to stderr) | Start-up, passkey enrolled (key name), CLI token created (name), update signed/sent, pairing (machine name, OS user), SSH install target and failure text, push/save failures | Machine names, OS user names, SSH targets. Never tokens, keys or passkey material | systemd user journal of the hub account | journald default |
| `atlas-hub setup` / `pair` (terminal) | One-time setup link / QR code, SSH output | The setup link is a short-lived secret, shown only on the owner's terminal | Owner's terminal | Not stored |
| `atlasd` | What it linked or wrote, problems found | Local paths | systemd user journal on Linux; `~/Library/Logs/atlasd.log` (0600) on macOS | journald default; the macOS file grows until deleted |

## Client-side storage

| Key | Holds | Lifetime | Cleared on sign-out? |
| --- | --- | --- | --- |
| `atlas.rail` (hub, localStorage) | Whether the side rail is collapsed | Until cleared | No (not personal) |
| `atlas-stars` (public site, localStorage) | Cached GitHub star count and time | 1 hour cache | n/a (no sign-in) |
| Hub session cookie | Passkey session | Session | Yes |

## Third parties

| Service | Receives | Why | Self-hostable? |
| --- | --- | --- | --- |
| Tailscale | Hub traffic metadata, the viewer's login (`Tailscale-User-Login`) | Network access and identity for the hub | Headscale |
| GitHub (owner's repos) | Pushed commits of the owner's knowledge and project repos | Storage and sync the owner chose | Any git host |
| GitHub API (`api.github.com`, public site only) | The visitor's IP and user agent, from their browser | Star count on the landing page | n/a; could be removed |
| Cloudflare (public site only) | Visitor requests to the static landing page | Hosting the site | Any static host |

## Usage and error telemetry

None. Atlas counts nothing and reports no errors anywhere except its own logs.

## Secrets

| Secret | Lives in | Rotate when |
| --- | --- | --- |
| MCP API keys (phase 4) | Hub keystore (age-encrypted) | Provider compromise or a hub leak |
| Keystore key | Hub, plus the owner's offline backup | Hub compromise |
| Client / CLI tokens | Hashes on the hub; plaintext only on each client (`~/.config/atlas`) | Unpairing or a lost machine |
| Passkey sessions | `sessions.json` on the hub (0600, SHA-256 of each id, the passkey that opened it), 12 h | Sign-out, Sign out everywhere, removing that passkey, expiry |

## Open findings

Not kept in this repository. Unfixed findings live in the maintainer's private register until
they're fixed, so this file never advertises an open hole.

## Accepted risks

- No HSTS header on the hub: it is reachable only over HTTPS on the tailnet (`tailscale serve`),
  with no HTTP listener to downgrade to.
