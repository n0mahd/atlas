// Client API: machines and the workstation CLI reach the hub with bearer tokens. Attacker:
// another device on the owner's tailnet (so the Tailscale login passes) without a valid
// token, with the wrong kind of token, or with a token from a machine the owner unpaired.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { machine } from "../../lib/world.mjs";
import { atlasd, pairCodeFromUi, pairedMachine, request } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "client-api" } });
test.describe.configure({ mode: "serial" });

const t = {};
const report = JSON.stringify({ release: 99, at: 0, harnesses: [], problems: ["forged"] });
const routes = {
  client: [
    ["GET", "/api/release"],
    ["POST", "/api/report", report],
  ],
  admin: [
    ["GET", "/api/admin/machines"],
    ["POST", "/api/admin/machines", JSON.stringify({ name: "evil" })],
    ["DELETE", "/api/admin/machines/laptop"],
    ["POST", "/api/admin/pair-code"],
  ],
};

const call = (world, [method, p, body], authorization) =>
  request(world, {
    method,
    path: p,
    headers: { ...(authorization === undefined ? {} : { authorization }), ...(body ? { "content-type": "application/json" } : {}) },
    body,
  });

test("setup: a paired laptop and a CLI token, made the owner's way", async ({ owner, world }) => {
  const laptop = await pairedMachine(world, "laptop", await pairCodeFromUi(owner));
  t.laptop = laptop;
  t.client = /token = "([0-9a-f]{64})"/.exec(laptop.read(".config/atlas/client.toml"))[1];
  await owner.goto("/settings#cli-tokens");
  await owner.getByPlaceholder("Name this workstation").fill("workstation");
  await owner.getByRole("button", { name: "Create token" }).click();
  t.cli = /atlasd auth \S+ ([0-9a-f]{64})/.exec(await owner.locator("#cli-tokens pre.ink-block").innerText())[1];
  // Controls: each token works where it belongs.
  expect((await call(world, routes.client[0], `Bearer ${t.client}`)).status).toBe(204);
  expect((await call(world, routes.admin[0], `Bearer ${t.cli}`)).status).toBe(200);
});

test("missing, malformed and unknown tokens are refused on every API route", async ({ world }) => {
  const clients = fs.readFileSync(path.join(world.state, "clients.json"), "utf8");
  const bad = {
    none: undefined,
    empty: "",
    "bare scheme": "Bearer ",
    "not hex": `Bearer ${"z".repeat(64)}`,
    short: `Bearer ${t.client.slice(0, 63)}`,
    long: `Bearer ${t.client}0`,
    unknown: `Bearer ${crypto.randomBytes(32).toString("hex")}`,
    basic: `Basic ${Buffer.from(`x:${t.client}`).toString("base64")}`,
    "token without scheme": t.client,
  };
  for (const route of [...routes.client, ...routes.admin]) {
    for (const [what, authorization] of Object.entries(bad)) {
      const res = await call(world, route, authorization);
      expect(res.status, `${route[0]} ${route[1]} with ${what}`).toBe(401);
    }
  }
  expect(fs.readFileSync(path.join(world.state, "clients.json"), "utf8"), "no report recorded").toBe(clients);
});

test("a token works only for its own API", async ({ world }) => {
  for (const route of routes.admin) expect((await call(world, route, `Bearer ${t.client}`)).status, `client token on ${route[1]}`).toBe(401);
  for (const route of routes.client) expect((await call(world, route, `Bearer ${t.cli}`)).status, `CLI token on ${route[1]}`).toBe(401);
});

test("pairing refuses a made-up code, and a wrong hub address doesn't spend a real one", async ({ owner, world }) => {
  const pair = (code, hub) =>
    request(world, {
      method: "POST",
      path: "/api/pair",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code, hub, machine: "evil", user: "x" }),
    });
  expect((await pair(crypto.randomBytes(32).toString("hex"), world.url)).status).toBe(403);
  expect((await pair("not-a-code", world.url)).status).toBe(403);
  const code = await pairCodeFromUi(owner);
  expect((await pair(code, "https://evil.example")).status).toBe(400);
  const m = await pairedMachine(world, "desk", code);
  expect(m.exists(".config/atlas/client.toml")).toBe(true);
});

test("a machine the owner unpairs loses access at once", async ({ owner, world }) => {
  await owner.goto("/machines");
  const ledger = owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "laptop" }) });
  await ledger.getByRole("button", { name: /^Unpair / }).click();
  await expect(owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "laptop" }) }).getByRole("button", { name: /^Unpair / })).toHaveCount(0);
  await shot(owner, "laptop unpaired");
  for (const route of routes.client) expect((await call(world, route, `Bearer ${t.client}`)).status, route[1]).toBe(401);
  const run = await atlasd(t.laptop, "run");
  expect(run.out).toMatch(/isn't paired with/);
  // The other paired machine still works.
  const desk = machine(world, "desk");
  const token = /token = "([0-9a-f]{64})"/.exec(desk.read(".config/atlas/client.toml"))[1];
  expect((await call(world, routes.client[0], `Bearer ${token}`)).status).toBe(204);
});
