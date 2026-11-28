// Send: saved changes become a numbered update that the owner's passkey signs. Knowledge is
// pushed first when it has unpushed commits; public skills never are; with nothing new
// there is no tap and no update; a closed passkey prompt sends nothing.

import fs from "node:fs";
import { test, expect, shot } from "../../lib/owner.mjs";
import { git } from "../../lib/world.mjs";
import { commitFiles, remoteHead, updateCount, sendNow } from "../../lib/distribute.mjs";

test.use({ worldOptions: { tag: "send" } });

const signCount = async (auth) => (await auth.credentials())[0].signCount;

test("Send on the front page pushes Knowledge and signs the next update", async ({ owner, world, auth }) => {
  const before = await updateCount(owner);
  const head = commitFiles(world, world.knowledge, { "memory/front-send.md": "---\nname: front-send\ndescription: A note sent from Home\nmetadata:\n  type: project\n  host: all\n---\nSent from Home.\n" }, "Add a note by hand");
  expect(remoteHead(world, "Knowledge"), "not pushed yet").not.toBe(head);

  await owner.goto("/");
  const needs = owner.locator("section[data-passkey]");
  await expect(needs).toContainText(/Nothing has been sent to your machines yet|aren't on your machines yet|isn't on your machines yet/);
  await shot(owner, "home with changes waiting");
  const taps = await signCount(auth);
  await needs.getByRole("button", { name: "Send" }).click();
  await owner.waitForURL(/\/\?sent=1/);
  await expect(owner.locator(".flash")).toHaveText("On your machines within a minute");
  await expect(owner.locator("section[data-passkey]")).toHaveCount(0);
  await shot(owner, "home after sending");
  expect(await signCount(auth), "one passkey tap").toBe(taps + 1);

  expect(remoteHead(world, "Knowledge"), "Send pushed Knowledge").toBe(head);
  expect(await updateCount(owner)).toBe(before + 1);
  await expect(owner.locator("#updates .rows")).toContainText(`Update ${before + 1} · `);
  await expect(owner.locator("#updates .rows li").first()).toContainText("Verify laptop");
  await expect(owner.locator("#updates")).toContainText("Your machines have every saved change");
  await shot(owner, "settings lists the update");
  expect(fs.readFileSync(world.hubLog, "utf8")).toContain(`update ${before + 1} signed and sent`);
});

test("Save sends: one tap saves the note and sends it", async ({ owner, world, auth }) => {
  const before = await updateCount(owner);
  await owner.goto("/memory/user-tz/edit");
  const text = owner.locator("#editor-text");
  await text.fill((await text.inputValue()).replace("Eastern.", "Eastern, saved and sent."));
  await shot(owner, "editing a memory note");
  const taps = await signCount(auth);
  await owner.getByRole("button", { name: "Save" }).click();
  await owner.waitForURL(/sent=1/);
  await expect(owner.locator(".flash")).toHaveText("Saved · on your machines within a minute");
  await shot(owner, "saved and sent");
  expect(await signCount(auth), "the save and the send share one tap").toBe(taps + 1);

  const head = git(world, world.knowledge, "rev-parse", "HEAD");
  expect(remoteHead(world, "Knowledge")).toBe(head);
  expect(git(world, world.knowledge, "show", "HEAD:memory/user-tz.md")).toContain("Eastern, saved and sent.");
  expect(await updateCount(owner)).toBe(before + 1);
});

test("Send with nothing new asks for no tap and adds no update", async ({ owner, auth }) => {
  await sendNow(owner); // whatever is waiting goes first
  const before = await updateCount(owner);
  await expect(owner.locator("#updates")).toContainText("Your machines have every saved change");
  await owner.goto("/");
  await expect(owner.locator("section[data-passkey]"), "Home offers no Send").toHaveCount(0);

  const taps = await signCount(auth);
  await sendNow(owner);
  await shot(owner, "send with nothing new");
  expect(await signCount(auth), "no tap").toBe(taps);
  expect(await updateCount(owner)).toBe(before);
  await expect(owner.locator("#updates")).toContainText("Your machines have every saved change");
});

test("Closing the passkey prompt sends nothing", async ({ owner, world, auth }) => {
  const before = await updateCount(owner);
  commitFiles(world, world.knowledge, { "memory/cancelled.md": "---\nname: cancelled\ndescription: Not sent\nmetadata:\n  type: project\n  host: all\n---\nWaits.\n" }, "A change to cancel");
  await owner.goto("/");
  await auth.refuse();
  try {
    await owner.locator("section[data-passkey]").getByRole("button", { name: "Send" }).click();
    await expect(owner.getByRole("alert")).toHaveText("The passkey prompt was closed. Try again.");
    await shot(owner, "passkey prompt closed on Home");
    expect(await updateCount(owner)).toBe(before);

    // Save sends, cancelled at the tap: the save stays, the send waits.
    await owner.goto("/memory/verify-outcomes/edit");
    const text = owner.locator("#editor-text");
    await text.fill((await text.inputValue()) + "\nAlso when cancelled.\n");
    await owner.getByRole("button", { name: "Save" }).click();
    await owner.waitForURL(/notsent=1/);
    await expect(owner.locator(".flash")).toContainText("Saved. Not on your machines yet");
    await shot(owner, "saved, not sent");
    expect(git(world, world.knowledge, "show", "HEAD:memory/verify-outcomes.md")).toContain("Also when cancelled.");
    expect(await updateCount(owner)).toBe(before);
  } finally {
    await auth.refuse(false);
  }

  // The flash's Send button sends it once the owner taps.
  await owner.goBack();
  await owner.locator(".flash").getByRole("button", { name: "Send" }).click();
  await owner.waitForURL(/sent=1/);
  await expect(owner.locator(".flash")).toContainText("on your machines within a minute");
  await shot(owner, "sent from the flash");
  expect(await updateCount(owner)).toBe(before + 1);
});

test("Public skills reach machines only after the owner pushes them", async ({ owner, world }) => {
  await sendNow(owner);
  const before = await updateCount(owner);
  const head = commitFiles(world, world.publicSkills, { "review/SKILL.md": "---\nname: review\ndescription: Review code for correctness and tests\n---\n# review\n\nCheck the tests too.\n" }, "Public skill edit, not pushed");
  const pushedBefore = remoteHead(world, "Skills");

  await owner.goto("/settings");
  await owner.locator("#updates").getByRole("button", { name: "Send now" }).click();
  await expect(owner.locator("#updates").getByText("Public skills reach your machines after you push them.")).toBeVisible();
  await shot(owner, "public skills note");
  expect(remoteHead(world, "Skills"), "Atlas never pushes public skills").toBe(pushedBefore);
  expect(await updateCount(owner)).toBe(before);

  git(world, world.publicSkills, "push", "-q", "origin", "main");
  expect(remoteHead(world, "Skills")).toBe(head);
  await owner.waitForTimeout(3100); // the store snapshot is reused for 3 s
  await owner.goto("/");
  await expect(owner.locator("section[data-passkey]")).toContainText("1 change isn't on your machines yet");
  await sendNow(owner);
  expect(await updateCount(owner)).toBe(before + 1);
  await shot(owner, "public skills sent after the push");
});

test("the old Releases page leads to Settings → Updates", async ({ owner }) => {
  await owner.goto("/releases");
  expect(new URL(owner.url()).pathname).toBe("/settings");
  await expect(owner.locator("#updates")).toBeVisible();
});

