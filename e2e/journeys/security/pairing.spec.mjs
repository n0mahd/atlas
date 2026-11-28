// Pairing codes: a machine pairs with a one-time code from the Machines page. The code
// works once, for 15 minutes, and a wrong one gets nothing. Attacker: another device on
// the owner's tailnet guessing codes, or replaying one seen in a terminal. The last test
// runs the hub on a test clock (lib/clock-shim.c).

import crypto from "node:crypto";
import { test, expect, shot, signIn } from "../../lib/owner.mjs";
import { machine } from "../../lib/world.mjs";
import { advanceClock, atlasd, pairCodeFromUi, pairedMachine, request, restartHubWithClock } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "pairing" } });
test.describe.configure({ mode: "serial" });

const REFUSED = "That pairing code is wrong, has expired or was already used";
const pairApi = (world, code, hub = world.url) =>
  request(world, {
    method: "POST",
    path: "/api/pair",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code, hub, machine: "intruder", user: "x" }),
  });

async function expectPairRefused(world, name, code) {
  const m = machine(world, name);
  const r = await atlasd(m, "pair", world.url, code, "--machine", name);
  expect(r.code, r.out).not.toBe(0);
  expect(r.out).toContain(REFUSED);
  expect((await atlasd(m, "status")).code, "the machine isn't paired").not.toBe(0);
}

test("a pairing code works once", async ({ owner, world }) => {
  const code = await pairCodeFromUi(owner);
  await expect(owner.getByText("The code works once, for 15 minutes.")).toBeVisible();
  await shot(owner, "pairing command");
  await pairedMachine(world, "laptop", code);
  await expectPairRefused(world, "replay", code);
  expect((await pairApi(world, code)).status).toBe(403);
  await owner.goto("/machines");
  await expect(owner.getByRole("heading", { name: "replay" })).toHaveCount(0);
  await expect(owner.getByRole("heading", { name: "intruder" })).toHaveCount(0);
});

test("a wrong code gets nothing", async ({ world }) => {
  await expectPairRefused(world, "guess", crypto.randomBytes(32).toString("hex"));
  // Decision 0039 limits wrong codes hub-wide (pairing-limit.spec.mjs), and every refused
  // attempt in this file counts, so only one more here.
  for (const code of ["0000-000000"]) {
    const res = await pairApi(world, code);
    expect(res.status, JSON.stringify(code)).toBe(403);
    expect(res.body).not.toMatch(/token/);
  }
});

test("a wrong hub address is refused without spending the code", async ({ owner, world }) => {
  const code = await pairCodeFromUi(owner);
  const res = await pairApi(world, code, "https://evil.example");
  expect(res.status).toBe(400);
  await pairedMachine(world, "desk", code);
});

test("a pairing code expires after 15 minutes", async ({ owner, world }) => {
  await restartHubWithClock(world);
  // Decision 0034: the session survives the restart, so no new sign-in.
  const late = await pairCodeFromUi(owner);
  const inTime = await pairCodeFromUi(owner);
  advanceClock(world, 14 * 60);
  await pairedMachine(world, "in-time", inTime);
  advanceClock(world, 60 + 5);
  await expectPairRefused(world, "late", late);
});
