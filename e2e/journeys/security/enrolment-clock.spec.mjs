// Enrolment against the clock: the first-run window closes after first_run_minutes, and
// a setup link stops working after 15 minutes. The hub runs on a test clock
// (lib/clock-shim.c) so time can pass without waiting. Attacker: another device on the
// owner's tailnet arriving after the window, or someone with an old setup link.

import { test, expect, shot } from "../../lib/owner.mjs";
import { advanceClock, hubCliWithClock, request, restartHubWithClock } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "enrolment-clock" }, enrol: false });
test.describe.configure({ mode: "serial" });

const enrolStart = (world, token = "") =>
  request(world, {
    method: "POST",
    path: "/auth/enrol/start",
    headers: { "content-type": "application/json", origin: world.url },
    body: JSON.stringify({ token, name: "late" }),
  });

const setupLink = (world) => /(http\S+\/setup\/([0-9a-f]{64}))/.exec(hubCliWithClock(world, "setup")).slice(1);

test("the first-run window closes after 30 minutes", async ({ owner, world }) => {
  await restartHubWithClock(world);
  advanceClock(world, 29 * 60);
  await owner.goto("/signin");
  await expect(owner.getByText("Open for 1 more minute.")).toBeVisible();
  await shot(owner, "one minute left");
  advanceClock(world, 2 * 60);
  await owner.goto("/signin");
  await expect(owner.getByText("its first-run window has closed")).toBeVisible();
  await expect(owner.getByRole("button", { name: "Create passkey" })).toHaveCount(0);
  await shot(owner, "first-run window closed");
  const res = await enrolStart(world);
  expect(res.status).toBe(403);
  expect(res.body).toContain("The first-run window has closed");
});

test("a setup link expires after 15 minutes", async ({ world }) => {
  const [, token] = setupLink(world);
  advanceClock(world, 15 * 60 + 5);
  const page = await request(world, { path: `/setup/${token}` });
  expect(page.status).toBe(404);
  expect(page.body).toContain("expired or was already used");
  expect((await enrolStart(world, token)).status).toBe(403);
});

test("a fresh setup link still works after the window closed", async ({ owner, world }) => {
  const [url] = setupLink(world);
  advanceClock(world, 14 * 60);
  await owner.goto(url);
  await owner.getByPlaceholder(/Laptop, phone/).fill("Owner laptop");
  await owner.getByRole("button", { name: "Create passkey" }).click();
  await owner.waitForURL(/\/settings/);
  await expect(owner.getByText("Owner laptop")).toBeVisible();
  await shot(owner, "passkey added through a setup link");
});
