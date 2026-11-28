# Atlas user guide

Atlas keeps your whole agent setup in one place: skills, instruction files, memory and
hooks, with MCP servers to follow. You edit it once, on your hub, and Atlas carries it to
every machine and every agent you use there: Claude Code, Codex, OpenCode, Pi,
Antigravity, Amp, Muse and Grok Build.

## How it fits together

- **The hub** runs on one machine you pick. You open its web page from any device on your
  Tailscale network. Nothing is on the public internet.
- **atlasd** runs on each machine, as your own account. Every 10 minutes a systemd user
  timer starts it; it fetches the latest update and links skills, instructions and hooks
  into each agent's own folders.
- **Everything is stored in git.** Every save is a commit, so nothing is lost and any
  change can be undone.
- **Your passkey authorises changes.** Reading needs only your Tailscale login. Saving,
  sending and pairing need a passkey tap.
- **Save sends.** One passkey tap saves your edit, pushes it and sends an update. Your
  machines pick it up within 10 minutes.

## Pages

1. [Install the hub](install-hub.md): once, on the machine that will hold everything.
2. [Add a machine](add-a-machine.md): one command per machine and account.
3. [Editing and updates](editing-and-updates.md): what Save does and where changes go.
4. [Troubleshooting](troubleshooting.md): common problems and their fixes.
5. [Security](security.md): who can change what, in plain terms.

## What works today

- Viewing and editing skills, instruction files and memory in the hub.
- Hooks are shown, and atlasd installs them, but you edit them in the repo, not the hub.
- atlasd on Linux (x86_64 and ARM) and macOS (Apple silicon and Intel).
- MCP servers aren't managed yet. The MCP page says so.
