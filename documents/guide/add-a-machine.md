# Add a machine

Each machine needs atlasd, paired with your hub. Do this once per machine, and once per
account on it whose agents Atlas should manage. That includes the hub machine itself if
you run agents there.

## Before you start

- The machine runs Linux (x86_64 or ARM) with systemd, or macOS (Apple silicon or Intel).
- It is on your tailnet, signed in to Tailscale as a login the hub allows.
- It has `git`, and can fetch your knowledge repo from its remote (for a private GitHub
  repo, an SSH key or credentials that work for `git clone`).
- The hub has atlasd builds to hand out ([install the hub](install-hub.md)).

## Get the command

On the hub's page, sign in, open **Machines** and press **Pair**. Atlas shows a command
like this:

```sh
curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- 3f9c2a…
```

The code at the end works once, for 15 minutes. Press **Pair** again for a new one.

Then pick one of the three ways below.

## 1. On the machine

Open a terminal on the machine, as the account whose agents Atlas should manage (not
root), paste the command and press Enter.

## 2. From another computer, over SSH

Run the same command through `ssh`, as that account:

```sh
ssh you@laptop 'curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- 3f9c2a…'
```

Keep the single quotes, so the whole command runs on the other machine.

## 3. From the hub, without a terminal

- In the hub: **Machines**, then the "Or install it from here over SSH" row. Enter
  `you@laptop` and press **Install**. The page shows the result, and the output if it
  failed. It can take up to a few minutes; leave the page open.
- Or, on the hub machine: `atlas-hub add-machine you@laptop`. To just print a fresh
  install command to copy yourself: `atlas-hub pair-code`.

Both need the hub to reach the machine over SSH: either Tailscale SSH turned on there
(run `sudo tailscale set --ssh` on the machine once), or the hub's SSH key in that
account's `~/.ssh/authorized_keys`.

## What the installer does

1. Checks the machine is Linux and picks the build for its CPU.
2. Downloads atlasd from the hub and checks its SHA-256, so a damaged or altered download
   is refused.
3. Installs it as `~/.local/bin/atlasd`.
4. Pairs it with the hub, using the one-time code. The machine gets its own token.
5. If `~/Knowledge` or `~/Skills` exist, lets atlasd take over the skill links an older
   setup made into them.
6. Runs atlasd every 10 minutes. On Linux that's a systemd user timer, with "linger"
   turned on so it keeps running when you're logged out. On macOS it's a launchd agent,
   `~/Library/LaunchAgents/atlasd.plist`, logging to `~/Library/Logs/atlasd.log`.

atlasd runs as your account. It never asks for root.

## Check it worked

See what atlasd would change, without changing anything:

```sh
atlasd run --dry-run
```

(On macOS `~/.local/bin` usually isn't on your `PATH`; run `~/.local/bin/atlasd` instead.)

Before you've sent your first update it says the hub has no signed release yet. That's
fine: send one from **Settings → Updates** ([editing and updates](editing-and-updates.md)).

`atlasd status` shows the hub, the machine's name, the last update applied and the plan.

On the hub, **Machines** lists the machine and account, with "seen … ago" once atlasd has
reported in. If it says "Update 3, 1 behind", the machine hasn't picked up the latest yet;
wait for the next 10-minute run.

## Remove a machine

1. On the hub: **Machines**, then the × next to the account. Its token stops working at
   once.
2. On the machine, stop atlasd and remove its files:

   ```sh
   systemctl --user disable --now atlasd.timer
   rm ~/.config/systemd/user/atlasd.{service,timer} ~/.local/bin/atlasd
   rm -r ~/.config/atlas ~/.local/state/atlas-client
   ```

   On macOS, replace the first two lines with:

   ```sh
   launchctl bootout gui/$(id -u)/atlasd
   rm ~/Library/LaunchAgents/atlasd.plist ~/.local/bin/atlasd
   ```

The skill links atlasd made point into `~/.local/share/atlas/sources`. Delete that folder
only once you've replaced those links, or your agents lose those skills. Atlas can't yet
remove the line it added to Claude Code's `CLAUDE.md`; delete it by hand if you want.
