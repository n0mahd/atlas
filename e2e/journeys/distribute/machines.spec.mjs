// The Machines page: a one-time pairing command, tracking and forgetting machines, and
// unpairing a client, whose token then stops working.

import { test, expect, shot } from "../../lib/owner.mjs";
import { hubCli, machine } from "../../lib/world.mjs";
import { atlasd, installHarnesses, pairCode, pairMachine, sendNow } from "../../lib/distribute.mjs";

test.use({ worldOptions: { tag: "machines" } });

const ledgerOf = (owner, name) =>
  owner.locator("section.ledger", { has: owner.getByRole("heading", { name, exact: true }) });

test("Pair shows a one-time install command that works once", async ({ owner, world }) => {
  await owner.goto("/machines");
  await expect(ledgerOf(owner, "verify-hub").locator(".badge")).toHaveText("Hub");
  await expect(ledgerOf(owner, "laptop").locator(".ledger-sum")).toHaveText("Not paired yet");
  await shot(owner, "machines before pairing");

  const { command, code } = await pairCode(owner);
  expect(command).toBe(`curl -fsSL ${world.url}/install | sh -s -- ${code}`);
  await expect(owner.locator("pre.ink-block").nth(1)).toHaveText(`ssh you@machine '${command}'`);
  await expect(owner.locator(".copy-btn").first()).toHaveAttribute("data-copy", command);
  await expect(owner.getByText("The code works once, for 15 minutes.")).toBeVisible();
  await shot(owner, "pairing command");

  // Pair on a machine's own row: the SSH variant names that machine.
  const forLaptop = await pairCode(owner, { machine: "laptop" });
  await expect(owner.locator("pre.ink-block").nth(1)).toHaveText(`ssh laptop '${forLaptop.command}'`);
  expect(forLaptop.code).not.toBe(code);

  const a = machine(world, "first");
  const ok = await atlasd(a, ["pair", world.url, code, "--machine", "first"]);
  expect(ok.code, ok.out).toBe(0);
  const b = machine(world, "second");
  const again = await atlasd(b, ["pair", world.url, code, "--machine", "second"]);
  expect(again.code).not.toBe(0);
  expect(again.out).toContain("That pairing code is wrong, has expired or was already used");
  expect(b.exists(".config/atlas/client.toml")).toBe(false);

  const wrongHub = await atlasd(b, ["pair", world.url.replace("localhost", "127.0.0.1"), forLaptop.code, "--machine", "second"]);
  expect(wrongHub.out, "a wrong address is refused before the code is spent").toContain(`This hub's address is ${world.url}`);
  expect((await atlasd(b, ["pair", world.url, forLaptop.code, "--machine", "second"])).code).toBe(0);

  await owner.goto("/machines");
  await expect(ledgerOf(owner, "first").locator(".ledger-sum")).toHaveText("1 paired");
  await expect(ledgerOf(owner, "first").locator(".client-row")).toContainText("No update yet");
  await shot(owner, "two machines paired");
});

test("Track a machine by name, then forget it", async ({ owner }) => {
  await owner.goto("/machines");
  const track = owner.locator("form[action='/machines/add']");
  await track.getByLabel("Machine name").fill("studio");
  await track.getByRole("button", { name: "Add" }).click();
  await expect(ledgerOf(owner, "studio").locator(".ledger-sum")).toHaveText("Not paired yet");
  await expect(ledgerOf(owner, "studio").getByRole("button", { name: "Install" })).toBeVisible();
  await shot(owner, "studio tracked");

  await track.getByLabel("Machine name").fill("verify-hub");
  await track.getByRole("button", { name: "Add" }).click();
  await expect(owner.getByRole("status")).toHaveText("verify-hub is already this hub.");
  await owner.locator("form[action='/machines/add']").getByLabel("Machine name").fill("bad name!");
  await owner.locator("form[action='/machines/add']").getByRole("button", { name: "Add" }).click();
  await expect(owner.getByRole("status")).toContainText("isn't a valid machine name");
  await shot(owner, "refused machine names");

  await owner.goto("/machines");
  await owner.getByRole("button", { name: "Remove studio" }).click();
  await expect(owner.getByRole("heading", { name: "studio", exact: true })).toHaveCount(0);
  await shot(owner, "studio forgotten");
});

test("Unpair a client: its token stops working", async ({ owner, world }) => {
  const m = machine(world, "spare");
  installHarnesses(m, ["claude"]);
  await sendNow(owner);
  await pairMachine(owner, world, m);
  expect((await atlasd(m, ["run"])).code).toBe(0);
  const files = m.read(".claude/CLAUDE.md");

  await owner.goto("/machines");
  const ledger = ledgerOf(owner, "spare");
  await expect(ledger.locator(".client-row")).toContainText("up to date");
  await shot(owner, "spare paired and reporting");
  await ledger.getByRole("button", { name: /^Unpair .* on spare$/ }).click();
  await expect(owner.getByRole("heading", { name: "spare", exact: true })).toHaveCount(0);
  await shot(owner, "spare unpaired");

  const after = await atlasd(m, ["run"]);
  test.info().attach("atlasd after unpairing", { body: after.out });
  expect(after.out).toContain("this login isn't paired with");
  expect(m.read(".claude/CLAUDE.md"), "the machine's files stay").toBe(files);
});

test("Forgetting a tracked machine also unpairs its clients", async ({ owner, world }) => {
  await owner.goto("/machines");
  const track = owner.locator("form[action='/machines/add']");
  await track.getByLabel("Machine name").fill("desk");
  await track.getByRole("button", { name: "Add" }).click();
  const m = machine(world, "desk");
  const { code } = await pairCode(owner, { machine: "desk" });
  expect((await atlasd(m, ["pair", world.url, code, "--machine", "desk"])).code).toBe(0);
  await owner.goto("/machines");
  await expect(ledgerOf(owner, "desk").locator(".ledger-sum")).toHaveText("1 paired");
  await expect(ledgerOf(owner, "desk").getByRole("button", { name: "Install" }), "a paired machine offers no install").toHaveCount(0);

  await owner.getByRole("button", { name: "Remove desk" }).click();
  await expect(owner.getByRole("heading", { name: "desk", exact: true })).toHaveCount(0);
  expect((await atlasd(m, ["run"])).out).toContain("this login isn't paired with");
});

test("The open Machines page picks up a newly paired machine without a reload", async ({ owner, world }) => {
  // The pairing code comes from the hub's CLI, so the open page is never navigated.
  const code = hubCli(world, "pair-code").match(/sh -s -- (\S+)/)[1];
  await owner.goto("/machines");
  await owner.evaluate(() => (window.__notReloaded = true));
  await expect(ledgerOf(owner, "poller").locator(".ledger-sum")).toHaveCount(0);
  await shot(owner, "machines open, poller not paired");

  const m = machine(world, "poller");
  const paired = await atlasd(m, ["pair", world.url, code, "--machine", "poller"]);
  expect(paired.code, paired.out).toBe(0);

  // The list polls /machines/list every 10 s and swaps itself in place.
  await expect(ledgerOf(owner, "poller").locator(".ledger-sum")).toHaveText("1 paired", { timeout: 20000 });
  expect(await owner.evaluate(() => window.__notReloaded)).toBe(true);
  await shot(owner, "machines updated in place");
});
