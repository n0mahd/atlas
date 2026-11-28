# Add a machine

Each machine needs atlasd, paired with your hub. Do this once per machine, and once per
account on it whose agents Atlas should manage. That includes the hub machine itself if
you run agents there.

## Before you start

- The machine runs Linux (x86_64 or ARM) with systemd, or macOS (Apple silicon or Intel).
- It is on your Tailscale network (your tailnet), signed in as a login the hub allows.
  Devices with a Tailscale tag have no login, so the hub turns them away with a 403.
- It has `git`, and can fetch your knowledge repo from its remote (for a private GitHub
  repo, an SSH key or credentials that work for `git clone`).
- The hub has atlasd builds to hand out ([install the hub](install-hub.md)).

## Get the command

On the hub's page, sign in, open **Machines** and press **Pair**. Atlas shows a command
like this:

```sh
curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- K7QF-9XMR2D
```

The code at the end works once, for 15 minutes; capitals and dashes don't matter. Press **Pair** again for a new one.

Then pick one of the three ways below.

## 1. On the machine

Open a terminal on the machine, as the account whose agents Atlas should manage (not
root), paste the command and press Enter.

## 2. From another computer, over SSH

Run the same command through `ssh`, as that account:

```sh
ssh you@laptop 'curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- K7QF-9XMR2D'
```

Keep the single quotes, so the whole command runs on the other machine.

## 3. From the hub, without a terminal

- In the hub: **Machines**, then the "Or install it from the hub over SSH" row. Enter
  `you@laptop` and press **Install**. The page shows the result, and the output if it
  failed. It can take up to a few minutes; leave the page open.
- Or, on the hub machine: `atlas-hub add-machine you@laptop`. To just print a fresh
  install command to copy yourself: `atlas-hub pair-code`.

Both need the hub to reach the machine over SSH: either Tailscale SSH turned on there
(run `sudo tailscale set --ssh` on the machine once), or the hub's SSH key in that
account's `~/.ssh/authorized_keys`.

## What the installer does

1. Checks the machine is Linux or macOS, picks the build for its CPU, and checks it can run
   atlasd in the background (systemd user services on Linux, launchd on macOS). If it
   can't, or the hub has no build for it, it stops and says why. Nothing is installed or
   paired.
2. Downloads atlasd from the hub and checks its SHA-256, so a damaged or altered download
   is refused. If the hub refuses the download, you see the hub's message.
3. Installs it as `~/.local/bin/atlasd`.
4. Pairs it with the hub, using the one-time code. The machine gets its own token.
5. Runs atlasd once while you watch and prints a two- or three-line summary: what it
   linked for which agents, and whether anything of yours was replaced.
6. Then runs it every 10 minutes. On Linux that's a systemd user timer, set to keep
   running after you log out (if that can't be turned on, it says atlasd only runs while
   you're logged in). On macOS it's a launchd agent, `~/Library/LaunchAgents/atlasd.plist`,
   logging to `~/Library/Logs/atlasd.log`.

atlasd runs as your account. It never asks for root.

## Check it worked

See what atlasd would change, without changing anything:

```sh
atlasd run --dry-run
```

(On macOS `~/.local/bin` usually isn't on your `PATH`; run `~/.local/bin/atlasd` instead.)

Before you've sent your first update it says "The hub has no signed release yet"
(atlasd's word for an update). That's fine: send one from **Settings → Updates** ([editing and updates](editing-and-updates.md)).

`atlasd status` shows the hub, the machine's name, the last update applied and the plan.

On the hub, **Machines** lists the machine and account, with "seen … ago" once atlasd has
reported in. If it says "Update 3, 1 behind", the machine hasn't picked up the latest yet;
wait for the next 10-minute run.

## Removing Atlas

### Remove a machine

1. On the hub: **Machines**, then the × next to the account. Its token stops working at
   once.
2. On the machine, stop atlasd and remove its own files. This leaves
   `~/.config/atlas/hub.toml` alone, so it is safe on the hub machine too:

   ```sh
   systemctl --user disable --now atlasd.timer
   rm ~/.config/systemd/user/atlasd.{service,timer} ~/.local/bin/atlasd
   rm -f ~/.config/atlas/client.toml ~/.config/atlas/cli.toml
   rm -r ~/.local/state/atlas-client
   ```

   On macOS, replace the first two lines with:

   ```sh
   launchctl bootout gui/$(id -u)/atlasd
   rm ~/Library/LaunchAgents/atlasd.plist ~/.local/bin/atlasd
   ```

What stays behind, because your agents are still using it:

- **Links.** The skills, instruction files and subagents atlasd linked into each agent's
  folders (such as `~/.claude/skills/<name>` or `~/.codex/AGENTS.md`) point into
  `~/.local/share/atlas/sources`, atlasd's local copies of your repos. They keep working
  until you delete that folder. To keep a skill, replace its link with a real copy
  (`cp -rL ~/.claude/skills/<name> /tmp/<name>`, remove the link, move the copy back),
  or link it to your own checkout of the knowledge repo instead. Then delete
  `~/.local/share/atlas`. `find ~ -maxdepth 4 -lname '*/.local/share/atlas/*'` lists
  the links that are left.
- **Claude Code's `CLAUDE.md`.** atlasd added one line to `~/.claude/CLAUDE.md`, an
  `@` import of the instructions in `~/.local/share/atlas/sources`. Atlas can't remove
  it yet; delete that line by hand.
- **Hooks.** Hook entries atlasd wrote into an agent's own settings end in
  `# atlas:<name>` (Claude Code, Codex and others) or are keys named `atlas:<name>`
  (Antigravity). Delete those entries by hand; your own entries are untouched.

### Removing the hub

On the hub machine:

```sh
systemctl --user disable --now atlas-hub
tailscale serve --https=7443 off          # the port you chose
rm ~/.config/systemd/user/atlas-hub.service ~/.local/bin/atlas-hub
rm ~/.config/atlas/hub.toml
rm -r ~/.local/state/atlas-hub            # passkeys, updates, pairings, atlasd builds
```

Your knowledge repo is yours and stays where it is, with its whole history. Deleting the
state folder forgets your passkeys and every machine's pairing, so a new hub means
pairing each machine again. Until you remove atlasd from them, machines keep the setup
they last applied and report that they can't reach the hub; that is harmless. Turning
off "keep running after you log out" (`loginctl disable-linger`) is up to you, as other
services may rely on it.
