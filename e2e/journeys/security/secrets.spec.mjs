// Secrets: after a full Send, pairing, atlasd run and CLI use, no secret sits anywhere it
// shouldn't, and a CLI token revoked in Settings stops working at once. (Pairing codes:
// pairing.spec.mjs.)

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { machine } from "../../lib/world.mjs";
import { atlasd, filesContaining, ownerSecrets, pairCodeFromUi, pairedMachine, request, sendNow } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "secrets" } });
test.describe.configure({ mode: "serial" });

const s = {}; // the secrets the journey sees, by name

test("full path: send, pair, run, and a CLI token from Settings", async ({ owner, world }) => {
  await sendNow(owner);
  s.pairing = await pairCodeFromUi(owner);
  const laptop = await pairedMachine(world, "laptop", s.pairing);
  const run = await atlasd(laptop, "run");
  expect(run.code, run.out).toBe(0);
  s.client = /token = "([0-9a-f]{64})"/.exec(laptop.read(".config/atlas/client.toml"))[1];

  await owner.goto("/settings#cli-tokens");
  await owner.getByPlaceholder("Name this workstation").fill("workstation");
  await owner.getByRole("button", { name: "Create token" }).click();
  const command = await owner.locator("#cli-tokens pre.ink-block").innerText();
  s.cli = /atlasd auth \S+ ([0-9a-f]{64})/.exec(command)[1];
  await shot(owner, "CLI token shown once");
  const ws = machine(world, "workstation");
  const auth = await atlasd(ws, "auth", world.url, s.cli);
  expect(auth.code, auth.out).toBe(0);
  const list = await atlasd(ws, "machines");
  expect(list.code, list.out).toBe(0);
  expect(list.out).toContain("laptop");
  // Shown on one page view only (decision 0038): the next view of Settings doesn't show it.
  await owner.goto("/");
  await owner.goto("/settings#cli-tokens");
  await expect(owner.locator("#cli-tokens")).not.toContainText(s.cli);
});

test("no secret is written where it shouldn't be", async ({ owner, world, auth }) => {
  const [cred] = await auth.credentials();
  const priv = Buffer.from(cred.privateKey, "base64");
  const { d } = crypto.createPrivateKey({ key: priv, format: "der", type: "pkcs8" }).export({ format: "jwk" });
  const { cookie, csrf } = await ownerSecrets(owner);
  const nowhere = {
    "passkey private key (base64)": priv.toString("base64"),
    "passkey private key (base64url)": priv.toString("base64url"),
    "passkey private key (hex)": priv.toString("hex"),
    "passkey private scalar (hex)": Buffer.from(d, "base64url").toString("hex"),
    "passkey private scalar (base64url)": d,
    "session cookie": cookie.value,
    "used pairing code": s.pairing,
  };
  for (const [what, needle] of Object.entries(nowhere)) {
    expect(filesContaining(world.root, [needle]), what).toEqual([]);
  }
  // Decision 0034: sessions survive restarts in sessions.json, keyed by the cookie's hash.
  // The server must compare the CSRF token, so it is kept there in the clear, and only there.
  expect(filesContaining(world.root, [csrf]).map((h) => h.file), "CSRF token").toEqual([
    "home/.local/state/atlas-hub/sessions.json",
  ]);
  expect(fs.statSync(path.join(world.root, "home/.local/state/atlas-hub/sessions.json")).mode & 0o777).toBe(0o600);
  // Each token lives only in the one machine file that needs it, closed to others.
  const where = (needle) => filesContaining(world.root, [needle]).map((h) => h.file);
  expect(where(s.client)).toEqual(["machines/laptop/.config/atlas/client.toml"]);
  expect(where(s.cli)).toEqual(["machines/workstation/.config/atlas/cli.toml"]);
  for (const f of ["machines/laptop/.config/atlas/client.toml", "machines/workstation/.config/atlas/cli.toml"])
    expect(fs.statSync(path.join(world.root, f)).mode & 0o077, f).toBe(0);
  // The hub's own state is closed to other users.
  expect(fs.statSync(world.state).mode & 0o077).toBe(0);
  for (const f of fs.readdirSync(world.state, { recursive: true })) {
    // dist/ holds the atlasd builds the hub hands to any allowed login; not secret.
    if (f.startsWith("dist/")) continue;
    const st = fs.statSync(path.join(world.state, f));
    if (st.isFile()) expect(st.mode & 0o077, f).toBe(0);
  }
});

test("a CLI token revoked in Settings stops working", async ({ owner, world }) => {
  await owner.goto("/settings#cli-tokens");
  await owner.getByRole("button", { name: "Revoke workstation" }).click();
  await expect(owner.getByText("CLI token revoked.")).toBeVisible();
  await shot(owner, "CLI token revoked");
  const res = await request(world, { path: "/api/admin/machines", headers: { authorization: `Bearer ${s.cli}` } });
  expect(res.status).toBe(401);
  const list = await atlasd(machine(world, "workstation"), "machines");
  expect(list.code).not.toBe(0);
  expect(list.out).toMatch(/401/);
});
