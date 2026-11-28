// Pairing codes are short (decision 0039), so the hub limits guessing: five wrong codes in
// a rolling minute refuse even a right one, and twenty void every code. Attacker: another
// device on the owner's tailnet guessing codes. Runs the hub on a test clock, in its own
// world so no other journey's wrong codes count.

import { test, expect, shot } from "../../lib/owner.mjs";
import { advanceClock, pairCodeFromUi, pairedMachine, request, restartHubWithClock } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "pairing-limit" } });
test.describe.configure({ mode: "serial" });

const pairApi = (world, code) =>
  request(world, {
    method: "POST",
    path: "/api/pair",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code, hub: world.url, machine: "guesser", user: "x" }),
  });

const TOO_MANY = "Too many wrong codes. Wait a minute and try again.";

test("five wrong codes in a minute refuse even the right one, until the minute passes", async ({ owner, world }) => {
  await restartHubWithClock(world);
  const code = await pairCodeFromUi(owner);
  expect(code).toMatch(/^[0-9A-Z]{4}-[0-9A-Z]{6}$/);
  await shot(owner, "a short pairing code");
  for (let i = 0; i < 5; i++) expect((await pairApi(world, `ZZZZ-ZZZZZ${i}`)).status).toBe(403);
  const blocked = await pairApi(world, code);
  expect(blocked.status).toBe(429);
  expect(blocked.body).toContain(TOO_MANY);

  advanceClock(world, 65);
  // Redeeming ignores case and hyphens: the code as someone might type it.
  await pairedMachine(world, "typed", code.toLowerCase().replace("-", ""));
});

test("twenty wrong codes void every code", async ({ owner, world }) => {
  const before = await pairCodeFromUi(owner);
  // Four a minute, so the per-minute limit never answers first.
  for (let i = 0; i < 20; i++) {
    if (i % 4 === 0) advanceClock(world, 65);
    expect((await pairApi(world, `YYYY-YYYY${String(i).padStart(2, "0")}`)).status, `guess ${i + 1}`).toBe(403);
  }
  advanceClock(world, 65);
  expect((await pairApi(world, before)).status, "a code made before the guessing is void").toBe(403);
  const after = await pairCodeFromUi(owner);
  await pairedMachine(world, "after-void", after);
});
