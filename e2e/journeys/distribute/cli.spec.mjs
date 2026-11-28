// Managing machines from a workstation: a CLI token made in Settings (or with
// `atlas-hub token`), saved with `atlasd auth`, then `atlasd machines list/track/remove/add`
// and `atlasd pair-code`. Revoking the token locks the CLI out.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { machine } from "../../lib/world.mjs";
import { atlasd, hubCommand, pairMachine, publishBuild, writeFakeBin } from "../../lib/distribute.mjs";

test.use({ worldOptions: { tag: "cli" } });

test("a CLI token from Settings drives atlasd machines and pair-code", async ({ owner, world }) => {
  const ws = machine(world, "workstation");
  await owner.goto("/settings#cli-tokens");
  const form = owner.locator("form[action='/settings/tokens']");
  await form.getByLabel("Name the CLI token").fill("workstation");
  await form.getByRole("button", { name: "Create token" }).click();
  await expect(owner.locator("#cli-tokens [role=status]")).toContainText("Token for workstation (shown once)");
  const command = (await owner.locator("#cli-tokens pre.ink-block").textContent()).trim();
  expect(command).toMatch(new RegExp(`^atlasd auth ${world.url} \\S+$`));
  await shot(owner, "cli token created");
  await owner.goto("/settings");
  await expect(owner.locator("#cli-tokens pre.ink-block"), "the token shows once").toHaveCount(0);
  await expect(owner.locator("#cli-tokens .rows")).toContainText("workstation");

  const auth = await atlasd(ws, command.split(" ").slice(1));
  expect(auth.code, auth.out).toBe(0);
  expect(auth.out).toContain(`Saved CLI token for ${world.url} (hub: verify-hub`);
  expect(fs.statSync(path.join(ws.home, ".config/atlas/cli.toml")).mode & 0o777).toBe(0o600);

  // A paired machine shows in the list with its status.
  const laptop = machine(world, "laptop");
  await pairMachine(owner, world, laptop);
  let list = await atlasd(ws, ["machines", "list"]);
  expect(list.code, list.out).toBe(0);
  expect(list.out).toContain("verify-hub  [hub]");
  expect(list.out).toMatch(/laptop {2}\(1 paired\)\n {2}unknown {2}id=\S+ {2}no update yet/);

  const track = await atlasd(ws, ["machines", "track", "nas"]);
  expect(track.out).toContain("Added nas to the hub's machine list.");
  list = await atlasd(ws, ["machines"]);
  expect(list.out).toContain("nas  (not paired yet)");
  await owner.goto("/machines");
  await expect(owner.getByRole("heading", { name: "nas", exact: true })).toBeVisible();
  await shot(owner, "nas tracked from the CLI");

  const removed = await atlasd(ws, ["machines", "remove", "nas"]);
  expect(removed.out).toContain("Removed nas from the hub's machine list.");
  const id = list.out.match(/id=(\S+)/)[1];
  const unpaired = await atlasd(ws, ["machines", "remove", id]);
  expect(unpaired.out).toContain(`Unpaired 1 client(s) matching ${id}.`);
  expect((await atlasd(laptop, ["run"])).out).toContain("this login isn't paired with");
  const missing = await atlasd(ws, ["machines", "remove", "nowhere"]);
  expect(missing.code).not.toBe(0);
  expect(missing.out).toContain('No machine or paired client named \\"nowhere\\"'.replaceAll("\\", ""));

  const code = await atlasd(ws, ["pair-code"]);
  expect(code.code, code.out).toBe(0);
  const install = code.out.match(/curl -fsSL \S+\/install \| sh -s -- (\S+)/);
  expect(install, code.out).not.toBeNull();
  const fresh = machine(world, "from-cli");
  expect((await atlasd(fresh, ["pair", world.url, install[1], "--machine", "from-cli"])).code).toBe(0);

  // Revoking the token in Settings locks the CLI out.
  await owner.goto("/settings#cli-tokens");
  await owner.getByRole("button", { name: "Revoke workstation" }).click();
  await expect(owner.locator(".flash")).toHaveText("CLI token revoked.");
  await shot(owner, "cli token revoked");
  const locked = await atlasd(ws, ["machines", "list"]);
  expect(locked.code).not.toBe(0);
  expect(locked.out).toContain("Invalid or missing CLI token");
});

test("atlasd machines add installs over the workstation's own ssh", async ({ world, owner }) => {
  publishBuild(world);
  const fake = writeFakeBin(world.root);
  const ws = machine(world, "workstation2");
  const made = await hubCommand(world, "token", "workstation2");
  expect(made.code, made.out).toBe(0);
  const command = made.out.match(/atlasd auth \S+ \S+/)[0];
  expect((await atlasd(ws, command.split(" ").slice(1))).code).toBe(0);

  const target = machine(world, "workbench");
  const add = await atlasd(ws, ["machines", "add", "verify@workbench"], { path: fake.bin });
  test.info().attach("atlasd machines add", { body: add.out });
  expect(add.code, add.out).toBe(0);
  expect(add.out).toContain("Installed and paired on verify@workbench.");
  expect(fs.readFileSync(fake.log, "utf8")).toContain(`-o ConnectTimeout=10 -o StrictHostKeyChecking=accept-new -- verify@workbench curl -fsSL ${world.url}/install | sh -s -- `);
  expect(target.read(".config/atlas/client.toml")).toContain('machine = "workbench"');
  const list = await atlasd(ws, ["machines", "list"]);
  expect(list.out).toContain("workbench  (1 paired)");

  const bad = await atlasd(ws, ["machines", "add", "-oProxyCommand=x"], { path: fake.bin });
  expect(bad.code).not.toBe(0);
  expect(bad.out).toContain("isn't an SSH address");

  await owner.goto("/machines");
  await expect(owner.getByRole("heading", { name: "workbench", exact: true })).toBeVisible();
});

test("the admin API refuses calls without a CLI token", async ({ world }) => {
  const ws = machine(world, "stranger");
  fs.mkdirSync(path.join(ws.home, ".config/atlas"), { recursive: true });
  fs.writeFileSync(path.join(ws.home, ".config/atlas/cli.toml"), `hub = "${world.url}"\ntoken = "${"0".repeat(64)}"\n`);
  const r = await atlasd(ws, ["machines", "list"]);
  expect(r.code).not.toBe(0);
  expect(r.out).toContain("Invalid or missing CLI token");
  const none = await atlasd(machine(world, "nobody"), ["pair-code"]);
  expect(none.out).toContain("Create a CLI token in Settings → CLI tokens");
  for (const [method, p] of [["GET", "/api/admin/machines"], ["POST", "/api/admin/pair-code"], ["DELETE", "/api/admin/machines/laptop"]]) {
    const res = await fetch(`${world.url}${p}`, { method });
    expect(res.status, `${method} ${p}`).toBe(401);
  }
});
