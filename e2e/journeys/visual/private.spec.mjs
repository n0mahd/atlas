// The baselines in e2e/baselines are committed to a public repo, so the screens they record
// must not show anything about the machine that rendered them. The world is built for
// that: hub_name "verify-hub", the login "owner@github", machines named "laptop", paths
// shown relative to the world's own HOME, and the hub's URL (a localhost port) masked.
// This journey proves it on every baselined screen, light and dark being the same text.

import os from "node:os";
import { test, expect } from "../../lib/owner.mjs";
import { SCREENS } from "../../lib/screens.mjs";

test.use({ worldOptions: { purpose: "visual" } });

/** What would identify the machine or person running the suite. */
const PRIVATE = [
  ["hostname", os.hostname()],
  ["OS user", os.userInfo().username],
  ["real home", os.homedir()],
  ["real home's parent", os.homedir().split("/").slice(0, -1).join("/") + "/"],
].filter(([, v]) => v && v.length > 2 && v !== "/home/");

test("no baselined screen shows this machine's hostname, user or home path", async ({ owner, world }, info) => {
  test.skip(info.project.name === "phone", "the same text renders at every width");
  const found = [];
  for (const screen of SCREENS) {
    await owner.goto(screen.path);
    const html = await owner.content();
    for (const [what, value] of PRIVATE)
      if (html.includes(value)) found.push(`${screen.path}: ${what}`);
    // The world's own absolute paths would carry the real home too.
    if (html.includes(world.root)) found.push(`${screen.path}: an absolute path into the world`);
  }
  expect(found).toEqual([]);
});
