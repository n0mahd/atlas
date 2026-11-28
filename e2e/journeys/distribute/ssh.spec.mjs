// Installing atlasd over SSH from the hub: the Machines page and `atlas-hub add-machine`.
// The hub's PATH starts with a fake `ssh` that runs the install command locally as the
// machine named by the target (its own HOME and hostname, systemctl shimmed).

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { machine } from "../../lib/world.mjs";
import { atlasd, hubCommand, publishBuild, writeFakeBin } from "../../lib/distribute.mjs";

test.use({
  worldOptions: {
    tag: "ssh",
    seedWith: (w) => writeFakeBin(w.root),
    env: (w) => ({ PATH: `${path.join(w.root, "fakebin")}:${process.env.PATH}` }),
  },
});

const sshLog = (world) => fs.readFileSync(path.join(world.root, "ssh.log"), "utf8").trim().split("\n");
const ledgerOf = (owner, name) =>
  owner.locator("section.ledger", { has: owner.getByRole("heading", { name, exact: true }) });

test("Install over SSH from the Machines page", async ({ owner, world }) => {
  publishBuild(world);
  const m = machine(world, "box");
  await owner.goto("/machines");
  const form = owner.locator("form.setting-row[action='/machines/pair/ssh']");
  await form.getByLabel("The machine's SSH address").fill("verify@box");
  await shot(owner, "ssh address typed");
  await form.getByRole("button", { name: "Install" }).click();
  await expect(owner.getByRole("status")).toHaveText(
    "Installed and paired on verify@box. It shows here once atlasd reports, within a minute.",
    { timeout: 60000 },
  );
  await shot(owner, "installed over ssh");

  const line = sshLog(world).at(-1);
  expect(line).toMatch(
    new RegExp(`^-o BatchMode=yes -o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -- verify@box curl -fsSL ${world.url}/install \\| sh -s -- \\S+$`),
  );
  expect(m.read(".config/atlas/client.toml")).toContain('machine = "box"');
  expect(m.exists(".local/bin/atlasd")).toBe(true);

  await owner.goto("/machines");
  await expect(ledgerOf(owner, "box").locator(".ledger-sum")).toHaveText("1 paired");
  await expect(ledgerOf(owner, "box").getByRole("button", { name: "Install" })).toHaveCount(0);
  await shot(owner, "box paired");
  expect((await atlasd(m, ["status"])).out).toContain("Machine: box");
});

test("A machine's own Install button uses its name as the address", async ({ owner, world }) => {
  publishBuild(world);
  const m = machine(world, "laptop");
  await owner.goto("/machines");
  await ledgerOf(owner, "laptop").getByRole("button", { name: "Install" }).click();
  await expect(owner.getByRole("status")).toHaveText(/^Installed and paired on laptop\./, { timeout: 60000 });
  expect(sshLog(world).at(-1)).toContain("-- laptop curl -fsSL");
  expect(m.read(".config/atlas/client.toml")).toContain('machine = "laptop"');
  await owner.goto("/machines");
  await expect(ledgerOf(owner, "laptop").locator(".ledger-sum")).toHaveText("1 paired");
});

test("A failed SSH install explains itself and offers the command to run by hand", async ({ owner, world }) => {
  publishBuild(world);
  await owner.goto("/machines");
  const form = owner.locator("form.setting-row[action='/machines/pair/ssh']");
  await form.getByLabel("The machine's SSH address").fill("verify@ghost");
  await form.getByRole("button", { name: "Install" }).click();
  await expect(owner.getByRole("status")).toHaveText(
    "The hub couldn't reach verify@ghost. Check the name and that the machine is on and on the tailnet, then try again.",
    { timeout: 60000 },
  );
  await expect(owner.locator("pre.ink-block").first()).toContainText("Could not resolve hostname ghost");
  await expect(owner.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
  await expect(owner.locator("pre.ink-block", { hasText: "ssh verify@ghost '" })).toBeVisible();
  await expect(owner.locator("form.setting-row[action='/machines/pair/ssh']").getByLabel("The machine's SSH address")).toHaveValue("verify@ghost");
  await shot(owner, "ssh install failed");

  const before = sshLog(world).length;
  await owner.locator("form.setting-row[action='/machines/pair/ssh']").getByLabel("The machine's SSH address").fill("-oProxyCommand=x");
  await owner.locator("form.setting-row[action='/machines/pair/ssh']").getByRole("button", { name: "Install" }).click();
  await expect(owner.getByRole("status")).toContainText("isn't an SSH address");
  expect(sshLog(world).length, "a bad address never reaches ssh").toBe(before);
  await shot(owner, "bad ssh address refused");
});

test("atlas-hub add-machine installs over SSH from the hub's terminal", async ({ owner, world }) => {
  publishBuild(world);
  const m = machine(world, "clibox");
  const r = await hubCommand(world, "add-machine", "verify@clibox");
  test.info().attach("atlas-hub add-machine", { body: r.out });
  expect(r.code, r.out).toBe(0);
  expect(r.out).toContain("Installing atlasd on verify@clibox over SSH");
  expect(r.out).toContain("Paired clibox (verify)");
  expect(r.out).toContain("Installed and paired on verify@clibox.");
  expect(m.read(".config/atlas/client.toml")).toContain('machine = "clibox"');

  const bad = await hubCommand(world, "add-machine", "verify@nowhere");
  expect(bad.code).not.toBe(0);
  expect(bad.out).toContain("The hub couldn't reach verify@nowhere.");

  const code = await hubCommand(world, "pair-code");
  expect(code.code, code.out).toBe(0);
  expect(code.out).toMatch(new RegExp(`curl -fsSL ${world.url}/install \\| sh -s -- \\S+`));

  await owner.goto("/machines");
  await expect(ledgerOf(owner, "clibox").locator(".ledger-sum")).toHaveText("1 paired");
  await shot(owner, "clibox added from the terminal");
});
