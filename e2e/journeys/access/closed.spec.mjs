// A hub whose first-run window has closed with no passkey: the browser can't create one,
// and `atlas-hub setup` on the hub is the way in.

import { test, expect, shot, allowConsole } from "../../lib/owner.mjs";
import { hubCli } from "../../lib/world.mjs";

test.use({ enrol: false, worldOptions: { firstRunMinutes: 0 } });

test("a closed first run points to atlas-hub setup, and its link works", async ({ owner, world }) => {
  await owner.goto("/signin");
  await expect(owner.locator("header.masthead h1")).toHaveText("Welcome to Atlas");
  await expect(owner.locator(".signin-lede")).toHaveText("This hub has no passkey yet, and its first-run window has closed.");
  await expect(owner.locator(".signin pre.ink-block")).toHaveText("atlas-hub setup");
  await expect(owner.getByRole("button", { name: "Create passkey" })).toHaveCount(0);
  await shot(owner, "first run closed");

  // The hub refuses a first passkey without a link, even if asked directly.
  allowConsole(owner, /status of 403/);
  const res = await owner.request.post("/auth/enrol/start", {
    data: { name: "Sneaky", token: "" },
    headers: { origin: world.url },
  });
  expect(res.status()).toBe(403);
  expect(await res.text()).toContain("The first-run window has closed.");

  const link = hubCli(world, "setup").match(/https?:\/\/\S+\/setup\/\S+/)?.[0];
  expect(link).toBeTruthy();
  await owner.goto(link);
  await owner.getByPlaceholder(/Laptop, phone/).fill("Laptop");
  await owner.getByRole("button", { name: "Create passkey" }).click();
  await expect(owner).toHaveURL(/\/settings$/);
  const keys = owner.locator("section.section", { has: owner.locator("h2", { hasText: "Passkeys" }) });
  await expect(keys.locator("ul.rows .row-name")).toHaveText(["Laptop"]);
  await expect(keys.locator("ul.rows .row-desc")).toContainText("from a setup link");
  await expect(owner.locator('meta[name="atlas-csrf"]')).toHaveCount(1);
  await shot(owner, "enrolled through the setup link");
});
