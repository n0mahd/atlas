# Editing and updates

## What you can edit

Open **Skills**, **Instructions** or **Memory**, pick an item and press the pencil at
the top of its page. You need to be signed in with your passkey; Atlas asks if you
aren't.

Hooks are shown but not edited in the hub yet. Edit them in your knowledge repo
(`hooks/<name>/`) and commit as usual.

## Every save is a commit

Save writes the file and makes one git commit of exactly that file, named like
"Edit skill writing in Atlas". Nothing is lost: you can see or undo any change with git.

Atlas refuses a save, and keeps your text on screen, if:

- the file changed since you opened it, or
- the file has other changes you haven't committed outside Atlas, or
- a project repo isn't on its `main` branch.

The note under the editor says where the save goes.

## Save sends

With **Send changes to my machines when I save** on (the default), Save on a global skill,
instruction file or memory page asks for one passkey tap. That tap:

1. saves and commits the edit,
2. pushes your knowledge repo, so machines can fetch it,
3. builds an update and signs it with your passkey.

You'll see "Saved · on your machines within 10 minutes". Each machine's atlasd checks
every 10 minutes and applies it.

If you close the passkey prompt, the edit is still saved. The page says "Saved. Not on
your machines yet" and offers a **Send** button.

Next to the passkey prompt, one line says what you're signing, such as "Update 4 · 1 note
changed" or "Update 5 · AGENTS.md and 2 skills changed".

## Hooks and passkeys get a look first

Hooks run by themselves on every machine, and an update carries everything committed to
your knowledge repo, including what an agent or another machine committed there. So when
an update adds or changes a hook, Save (or **Send**) shows a check page before the passkey
tap:

- each hook's name, when it runs ("Before a tool runs · Bash"), and its script in full
  when it's new, or what changed when it's changed;
- any passkey the update adds or removes, such as "This update adds the passkey 'Phone'";
- one line such as "3 notes and 1 skill changed outside Atlas", with the names behind it.

Press **Send** to sign exactly what the page shows. Anything committed after the page
opened waits for the next update. **Not now** keeps your edit saved but not sent.

Removing a hook needs no look. Your first update lists every hook and every passkey your
machines will trust. If a hook's script isn't marked as runnable in git, the page says so,
and your machines skip that hook until you fix it (`chmod +x`, then commit). A hook whose
folder or script is a link, or holds one, is flagged the same way and skipped: what runs
must be in the hook's own folder.

Skills and subagents can declare hooks of their own in their frontmatter (a `hooks:`
block), which run while that skill or subagent is in use. When that block is new or
changes, the page lists it with the other hooks, as "Hooks in the skill pdf". For a skill
with such a block, any change in its folder (a script its hooks run, say) is shown the
same way.

## Settings → Updates

- **Send changes to my machines when I save.** Off means Save only saves. Send later with
  **Send now**.
- **Also push project repos after saving.** Pushes a project's `main` after each save to
  it. If that repo deploys from `main`, the push may deploy it.
- **Send now.** Sends everything saved since the last update, with one passkey tap. If
  nothing changed, it says "Your machines already have everything." and sends nothing.
- **The list below** shows each update sent, with its number, date and the passkey that
  signed it.

## What reaches your machines

| You edit | Where it's saved | How it reaches machines |
|---|---|---|
| Global skills, instructions | Your knowledge repo | The next update (Save sends) |
| Public skills | Your public skills repo | The next update, after you push that repo yourself |
| Memory | Your knowledge repo | Through git; atlasd doesn't install memory pages |
| Subagents (`agents/<name>.md`) | Your knowledge repo | The next update; atlasd links them for Claude Code only, for now |
| Project skills, instructions | That project's repo, on `main` | Through the project repo itself |

Atlas never pushes the public skills repo. Check it for private details, then push it by
hand. Until then Atlas says "Public skills reach your machines after you push them".

Project files are committed but not sent: they live in the project, and anyone who
pulls the project gets them.

Memory goes straight to git. Your agents also write memory themselves, without a passkey,
and read it from the knowledge repo on each machine. Saving a memory page in the hub
still sends an update, because the knowledge repo changed.

## "N changes aren't on your machines yet"

Home (with a **Send** button) and **Settings → Updates** count the commits in your
knowledge repo (and pushed public skills) since the last update. Memory your agents wrote counts too, since it's in
the same repo. Press **Send now** to send them.

"Nothing has been sent to your machines yet" means you have never sent an update. Send
one before your first machine can apply anything.
