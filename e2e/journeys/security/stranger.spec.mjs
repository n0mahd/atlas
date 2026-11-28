// Identity, from another person's tailnet device: tailscale serve (the proxy here) adds
// their login, which isn't in allowed_logins, and replaces any login the browser sends.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { request } from "../../lib/security.mjs";

const REFUSAL = "Atlas is open only to its owner's Tailscale login.";
const paths = ["/", "/skills", "/settings", "/signin", "/static/atlas.css", "/install", "/api/release", "/no-such-page"];

test.use({ worldOptions: { login: "stranger@github" }, enrol: false });

test("a login not in allowed_logins is refused on every route", async ({ world, page }) => {
  for (const p of paths) {
    const res = await request(world, { path: p });
    expect(res.status, p).toBe(403);
    expect(res.body).toContain(REFUSAL);
  }
  const res = await page.goto(world.url + "/");
  expect(res.status()).toBe(403);
  await expect(page.locator("body")).toContainText(REFUSAL);
  await shot(page, "a stranger is refused");
});

test("a browser can't forge the owner's login through the proxy", async ({ world, browser }, info) => {
  // Attacker: the stranger's browser sending the owner's login itself.
  const context = await browser.newContext({
    ...info.project.use,
    extraHTTPHeaders: { "Tailscale-User-Login": "owner@github" },
  });
  const page = await context.newPage();
  const res = await page.goto(world.url + "/skills");
  expect(res.status()).toBe(403);
  await expect(page.locator("body")).toContainText(REFUSAL);
  await shot(page, "forged login header is replaced by the proxy");
  await context.close();
  const raw = await request(world, { path: "/", headers: { "Tailscale-User-Login": "owner@github" } });
  expect(raw.status).toBe(403);
  // And no sign-in or first-run enrolment either.
  const enrol = await request(world, {
    method: "POST",
    path: "/auth/enrol/start",
    // No body: the login check refuses before any body is read, and the test proxy
    // can race a request body against a hub that already answered (502).
    headers: { "content-type": "application/json", origin: world.url, "Tailscale-User-Login": "owner@github", "content-length": "0" },
  });
  expect(enrol.status).toBe(403);
  const store = path.join(world.state, "passkeys.json");
  const keys = fs.existsSync(store) ? JSON.parse(fs.readFileSync(store, "utf8")).keys : [];
  expect(keys).toEqual([]);
});
