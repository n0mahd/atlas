# Troubleshooting

## Installing the hub

`deploy/install.sh` checks what it needs before building and says in one sentence what's
missing. The rest:

- **It stops at "Putting the hub on your Tailscale network".** Tailscale is waiting for
  you to turn on HTTPS certificates or Serve for your Tailscale network (your tailnet).
  Open the link it printed, approve, and it goes on.
- **"couldn't keep your services running after you log out".** Run
  `sudo loginctl enable-linger $USER`, or the hub stops each time you log out.
- **"The hub service isn't running".** The lines after it are from the hub's log; most
  often `hub.toml` has a mistake. Fix it, then `systemctl --user restart atlas-hub`.
- **"Couldn't tell which Tailscale login this machine uses".** The machine is a tagged
  device, which has no login. Write `~/.config/atlas/hub.toml` yourself from
  `deploy/hub.example.toml`, with your own login in `allowed_logins`.

## Adding a machine

### `command not found: atlasd`

atlasd isn't installed on this machine yet, or the shell can't see it.

- Use the whole install command from **Machines → Pair**
  (`curl -fsSL https://hub.example.ts.net:7443/install | sh -s -- <code>`). It installs
  atlasd and pairs it in one go. Don't run `atlasd pair` on its own first.
- atlasd lives in `~/.local/bin`. If that folder isn't on your `PATH`, run it as
  `~/.local/bin/atlasd`, or add the folder to `PATH` in your shell's startup file.

### "That pairing code has expired or was already used"

Codes work once, for 15 minutes. Press **Pair** on the hub again and use the new command.

### "This hub's address is …; pair using that"

The machine used a different address for the hub than its `public_url`, such as an IP
address or a short name. Use the command exactly as **Machines → Pair** shows it.

### `curl: (22) … error: 403`

The hub only answers its owner's Tailscale login. Check that the machine is on your
tailnet and signed in to Tailscale as a login listed in `allowed_logins`
(`tailscale status` shows who it is). Devices with a Tailscale tag have no login, so they
always get a 403; sign the machine in as yourself instead.

### `curl: (22) … error: 400`

Most often the hub has no atlasd builds yet. Put them in
`~/.local/state/atlas-hub/dist/` on the hub ([install the hub](install-hub.md)).

### "atlasd isn't available for …"

atlasd runs on Linux and macOS, on x86_64 and ARM (aarch64). Other systems and CPUs
aren't supported yet.

### "This hub has no atlasd build for … on …"

The hub has builds for other kinds of machine, but not this one. Add it to
`~/.local/state/atlas-hub/dist/` on the hub ([install the hub](install-hub.md#give-the-hub-atlasd-builds-to-hand-out)).

## Passkeys

### "The passkey prompt was closed. Try again."

You closed or cancelled the prompt, or it timed out. Nothing was changed. If you were
saving, the edit is saved; press **Send** on the page to send it.

### Save worked, but it says "Not on your machines yet" (Safari)

Some browsers, Safari in particular, only show a passkey prompt straight after a click.
After Save, the prompt may not appear. Press the **Send** button on the saved page; that
click opens it.

### "This browser can't use passkeys"

Use a current browser, on the hub's `https://` address. Passkeys don't work over plain
`http://` or from an IP address.

### Locked out

On the hub machine, run `atlas-hub setup` and open the link it prints (or scan the QR
code). It works once, for 15 minutes.

## A machine isn't picking up changes

First check the hub: **Machines** shows each account's last update and when it was last
seen. "Hasn't applied an update" or a growing "seen … ago" means atlasd isn't running or
can't reach the hub.

On the machine:

```sh
systemctl --user status atlasd.timer        # is the timer running?
journalctl --user -u atlasd -n 50           # what did the last runs say?
atlasd run --dry-run                        # run once now, changing nothing
```

On macOS:

```sh
launchctl print gui/$(id -u)/atlasd | grep -E 'state|last exit'   # is the agent loaded?
tail -n 50 ~/Library/Logs/atlasd.log                               # what did the last runs say?
~/.local/bin/atlasd run --dry-run
```

If the timer is inactive:

```sh
systemctl --user enable --now atlasd.timer
loginctl enable-linger "$USER"   # keep running after you log out
```

### "couldn't reach the hub … harnesses keep using the local copies"

The machine is offline, or the hub is down. That's safe: atlasd changes nothing, and your
agents keep working from the copies they already have. It catches up on the next run.

### "couldn't fetch the knowledge repo"

The machine needs `git` and access to your knowledge repo's remote (the same URL the
hub's copy uses). Check that `git clone <that URL>` works from this account.

### "refused release …"

atlasd calls an update a release in its messages. It checks every update's signature and
number. It refuses one that isn't signed by a passkey it trusts, or that is older than
one it already applied.

A machine trusts the passkeys you had when it paired. It learns a newer passkey from an
update signed by one it already trusts. So if you signed with a passkey added later, send
one update with an older passkey first. If that's lost, pair the machine again.

### Skill links keep changing back

Two tools are managing the same links. Usually it's an older link script or dotfiles
tool you set up before Atlas, run by hand, a timer or a sync job. On a machine with
atlasd, stop the old script from running; atlasd does that job now.

### "… is your own folder or file" or "… was left alone"

atlasd only changes links it made (or was told to take over). Your own folders, files and
links elsewhere are left alone and reported on the Machines page. Move yours aside if you
want Atlas to manage that place. To hand over links an older script made into a folder,
add that folder to `adopt` in `~/.config/atlas/client.toml`, for example
`adopt = ["~/Knowledge"]`. The installer does this for `~/Knowledge` and `~/Skills` when
they exist.

## Saving

- **"The file changed since you opened it."** Someone (or an agent) edited it meanwhile.
  Copy your text, reload and edit again.
- **"This file has uncommitted changes made outside Atlas."** Commit or discard those
  changes in the repo first.
- **"… is on <branch>, not main."** Project edits commit to `main`. Switch the repo back
  to `main`.
- **"This repo has no git name and email set."** Set `user.name` and `user.email` in that
  repo, then save again.
