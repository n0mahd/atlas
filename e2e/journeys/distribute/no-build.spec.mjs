// Decision 0041: a hub with no atlasd builds issues no pairing codes and says how to add a
// build, on the Machines page, from `atlas-hub pair-code` and in the /install script.

import { execFileSync } from "node:child_process";
import { test, expect, shot } from "../../lib/owner.mjs";
import { hubCli } from "../../lib/world.mjs";

test.use({ worldOptions: { noBuild: true } });

const NO_BUILDS = "This hub has no atlasd builds to hand out yet.";

test("with no build, /install, Pair and pair-code all say how to add one", async ({ owner, world }) => {
  const res = await fetch(`${world.url}/install`);
  // 200 on purpose: `curl -fsSL` would otherwise show only "error: 400".
  expect(res.status).toBe(200);
  const script = await res.text();
  expect(script).toContain(NO_BUILDS);
  let ran;
  try {
    execFileSync("sh", ["-c", script], { encoding: "utf8", stdio: "pipe" });
  } catch (e) {
    ran = e;
  }
  expect(ran?.status, "the script exits 1").toBe(1);
  expect(`${ran?.stdout}${ran?.stderr}`).toContain(NO_BUILDS);

  expect(() => hubCli(world, "pair-code")).toThrow(/no atlasd builds to hand out/);

  // The Machines page offers no Pair at all, and says why.
  await owner.goto("/machines");
  await expect(owner.getByText("No atlasd builds to hand out yet, so there is no install command.")).toBeVisible();
  await expect(owner.getByRole("button", { name: "Pair", exact: true })).toHaveCount(0);
  await expect(owner.locator("pre.ink-block")).toHaveCount(0);
  await shot(owner, "no build: no pairing code");
});
