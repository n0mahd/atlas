# Security

Atlas puts instructions, skills and hooks in front of your agents, and hooks run
commands. So the question that matters is: who can change what your machines get? The
short answer is only you, with your passkey.

## Two locks on the hub

1. **Your Tailscale login.** The hub isn't on the public internet. It answers only
   devices on your tailnet signed in as a login in `allowed_logins`. That's enough to
   read.
2. **Your passkey.** Every change (saving, sending, pairing a machine, adding or
   removing a passkey) also needs a passkey session. A passkey is held by your device
   (laptop, phone or security key) and never leaves it.

While a new hub has no passkey, it lets your Tailscale login create the first one for 30
minutes after it starts. After that, a new passkey needs `atlas-hub setup`, run on the
hub machine itself.

## The hub can't sign updates

An update is signed by your passkey, at the moment you tap it. The hub holds no key that
could sign one. If someone broke into the hub, they still couldn't send changes to your
machines without your passkey.

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

Hooks are commands your agents run. They reach a machine only inside an update you
signed.

## Each machine has its own token

Pairing uses a one-time code that expires after 15 minutes. The machine then gets its
own token; the hub keeps only a hash of it. Remove a machine on the **Machines** page and
its token stops working.

## What to keep in mind

- Anything running as your account on the hub machine can reach the hub's local socket.
  Keep that machine's account to yourself.
- The atlasd builds the hub hands out are checked by SHA-256, but not yet signed.
- Add a second passkey, so losing one device doesn't lock you out.
- MCP keys will live in the hub's keystore, never in git. MCP isn't managed yet.
