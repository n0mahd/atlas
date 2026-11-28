// Identity: the hub answers only requests that carry an allowed Tailscale login, and only
// `tailscale serve` (the proxy here) can put one there. The socket is the trust boundary.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { rawRequest } from "../../lib/world.mjs";

const REFUSAL = "Atlas is open only to its owner's Tailscale login.";
const paths = ["/", "/skills", "/settings", "/signin", "/static/atlas.css", "/install", "/api/release", "/no-such-page"];

test.use({ worldOptions: { label: "identity" } });

test("no Tailscale login: every route is refused", async ({ world }) => {
  // Attacker: a process that reaches the hub without tailscale serve's header.
  for (const p of paths) {
    const res = await rawRequest(world, { path: p });
    expect(res.status, p).toBe(403);
    expect(res.body).toContain(REFUSAL);
    expect(res.headers["content-security-policy"], p).toBeTruthy();
  }
  for (const p of ["/auth/signin/start", "/api/pair", "/settings/tokens"]) {
    const res = await rawRequest(world, { method: "POST", path: p, headers: { "content-type": "application/json" }, body: "{}" });
    expect(res.status, p).toBe(403);
  }
});

test("a login that differs from an allowed one is refused", async ({ world }) => {
  // (Spaces around a header value are not part of it in HTTP, so they are not tried.)
  for (const login of ["Owner@github", "owner@GitHub", "owner@github.evil", "owner", "owner@github,owner@github", ""]) {
    const res = await rawRequest(world, { path: "/", headers: { "tailscale-user-login": login } });
    expect(res.status, JSON.stringify(login)).toBe(403);
  }
});

test("the socket is closed to other users", async ({ world }) => {
  expect(fs.statSync(world.socket).mode & 0o777).toBe(0o600);
  expect(fs.statSync(path.dirname(world.socket)).mode & 0o777).toBe(0o700);
});
