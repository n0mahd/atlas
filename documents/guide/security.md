# Security

Atlas puts instructions, skills and hooks in front of your agents, and hooks run
commands. So the question that matters is: who can change what your machines get? The
short answer is only you, with your passkey, as long as the hub's own machine is safe.

## Two locks on the hub

1. **Your Tailscale login.** The hub isn't on the public internet. It answers only
   devices on your Tailscale network (your tailnet) signed in as a login in
   `allowed_logins`. Other people's logins and tagged devices get a 403. That's enough
   to read.
2. **Your passkey.** Every change (saving, sending, pairing a machine) also needs a
   passkey session. Adding or removing a passkey, and creating a CLI token, each ask for
   a fresh tap from a passkey you already have, even when you're signed in. A passkey is
   held by your device (laptop, phone or security key) and never leaves it.

   When a passkey is added, your next sign-in with another one says so once ("A passkey
   was added: Phone, …"). Removing a passkey signs out every browser it signed in, and
   **Settings → Passkeys → Sign out everywhere** signs out every browser at once.

While a new hub has no passkey, it lets your Tailscale login create the first one for 30
minutes after it starts. After that, a new passkey needs `atlas-hub setup`, run on the
hub machine itself.

## What the passkey protects, and what it doesn't

An update is signed by your passkey, at the moment you tap it. The hub holds no key that
could sign one. So nobody can send a change to your machines without getting you to tap:
not someone who steals your signed-in session, not another device on your tailnet, not
whoever can write to your GitHub repo.

That stops at the hub's own machine. The hub serves the page that asks for your tap, so a
hub machine or account that has itself been taken over can misuse your next tap. A
machine you pair while the hub is compromised also trusts what the hub gives it: the
installer, the atlasd build and the list of passkeys. Treat the hub's machine and account
as you would a password manager's: keep it updated, and keep other people and untrusted
software off that account. Signing on a device that shows what is being signed is
planned.

## Machines check every update

Each machine learns your passkeys when it pairs. On every run, atlasd:

- refuses an update that isn't signed by one of those passkeys,
- refuses an update older than the one it last applied, or a different one reusing its
  number, so an old update can't be replayed,
- checks every repo it downloads against the exact contents the update names, before
  using it.

## GitHub is treated as untrusted

Machines fetch your repos from their git remotes, such as GitHub. Atlas doesn't trust
that copy: if anything there differs from what your signed update names, atlasd refuses
it. Keep the knowledge repo private anyway; it holds your memory and instructions.

## atlasd stays in its lane

- It runs as your own account, never as root.
- It never runs the content it receives. It only writes links and config entries.
- It changes only entries it manages, and leaves your own files and hand edits alone.
- If the hub can't be reached, it changes nothing.

Hooks are commands your agents run, on every machine that gets them. They reach a
machine only inside an update you signed, and you see each new or changed hook, and any
change to the passkeys your machines trust, before you sign.

An update carries your knowledge repo as it is when you send, not only what you edited
in Atlas. Anything you or an agent committed to it some other way goes out with the next
update, hooks included.

## Each machine has its own token

Pairing uses a one-time code such as `K7QF-9XMR2D` that expires after 15 minutes. The hub
takes at most five wrong codes a minute, and after twenty wrong codes it cancels every
code it has handed out, so guessing one doesn't work. The machine then gets its own
token; the hub keeps only a hash of it. Remove a machine on the **Machines** page and
its token stops working.

## What to keep in mind

- Anything running as your account on the hub machine can reach the hub's local socket.
  Keep that machine's account to yourself.
- **Prebuilt releases and verification.** `deploy/get.sh` and `deploy/install.sh` check
  every downloaded file (the deploy archive, `atlas-hub` and each `atlasd` build) against
  `SHA256SUMS` from the same GitHub release. That catches truncated downloads or tampering
  in transit, not a compromised GitHub release itself. Each release file also carries a
  GitHub build-provenance attestation tying it to the commit and workflow that built it
  (`gh attestation verify <file> -R n0mahd/atlas`); the installers do not verify
  attestations automatically, and there is no separate owner signing key yet.
- The `atlasd` builds the hub hands out are checked by SHA-256 against the hub's own
  checksum, not signed with a separate key yet.
- Nothing yet scans skills, memory or hooks for risky instructions. Atlas shows them
  safely in its pages, but what they tell an agent to do is up to you to check.
- Add a second passkey, so losing one device doesn't lock you out.
- MCP servers aren't managed yet. When they are, their keys are planned to stay on the
  hub, never in git.
