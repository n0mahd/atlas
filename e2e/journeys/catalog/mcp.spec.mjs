// MCP: phase IV. Today the page only says so and reads no MCP config.

import { test, expect, shot } from "../../lib/owner.mjs";

test("the MCP page says servers arrive in phase IV", async ({ owner }) => {
  await owner.goto("/");
  const nav = owner.getByRole("link", { name: "MCP" }).first();
  // The rail marks the section as not built yet.
  await expect(nav).toHaveAttribute("data-later", "");
  await nav.click();
  await expect(owner).toHaveURL(/\/mcp$/);
  await expect(owner.locator("header.masthead h1")).toHaveText("MCP servers");
  await expect(owner.locator("main .marginalia")).toHaveText(
    "MCP servers arrive in phase IV, with the gateway and the keystore. Until then Atlas reads no MCP config, so no key can reach these pages.",
  );
  await expect(owner.locator("main .row")).toHaveCount(0);
  await expect(owner.locator(".edit-button")).toHaveCount(0);
  await shot(owner, "mcp");
});
