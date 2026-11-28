// Doctor: the world starts, every read-only screen renders for the owner with no console
// errors or CSP violations, and a stranger without the Tailscale identity is refused.

import { test, expect, shot } from "../lib/owner.mjs";
import { rawRequest } from "../lib/world.mjs";

const screens = ["/", "/skills", "/instructions", "/memory", "/hooks", "/mcp", "/machines", "/settings"];

test("every screen renders", async ({ owner }) => {
  for (const p of screens) {
    const res = await owner.goto(p);
    expect(res.status(), p).toBe(200);
    await expect(owner.locator("main, #main").first()).toBeVisible();
    await shot(owner, `screen ${p}`);
  }
});

test("no identity, no entry", async ({ world }) => {
  const res = await rawRequest(world, { path: "/" });
  expect(res.status).toBe(403);
});
