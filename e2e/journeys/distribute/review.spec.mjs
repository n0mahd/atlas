// "Check before sending" (decision 0040): an update that adds or changes hooks shows them
// before the passkey tap, and what is signed is exactly what the page showed.
// Attacker for the second journey: an agent or sync that writes a hook to the knowledge
// repo while the owner is looking at the page.

import { test, expect, shot, manualReview } from "../../lib/owner.mjs";
import { git, machine } from "../../lib/world.mjs";
import { atlasd, commitFiles, installHarnesses, pairMachine } from "../../lib/distribute.mjs";
import { ownerSecrets, request } from "../../lib/security.mjs";

test.use({ worldOptions: { tag: "review" } });
test.describe.configure({ mode: "serial" });

const hook = (name) => ({
  [`hooks/${name}/hook.toml`]: `description = "${name}"\nevent = "pre-tool-use"\nmatcher = "Bash"\nrun = "run.sh"\n`,
  [`hooks/${name}/run.sh`]: "#!/bin/sh\nexit 0\n",
});

/** Commits a hook as an owner would: its script marked executable (decision 0040). */
function commitHook(world, name, message) {
  commitFiles(world, world.knowledge, hook(name), message);
  git(world, world.knowledge, "update-index", "--chmod=+x", `hooks/${name}/run.sh`);
  git(world, world.knowledge, "commit", "-q", "--amend", "--no-edit");
}

const post = (world, route, body, { cookie, csrf }) =>
  request(world, {
    method: "POST",
    path: route,
    headers: { "content-type": "application/json", origin: world.url, cookie: `__Host-atlas=${cookie}`, "x-atlas-csrf": csrf },
    body: JSON.stringify(body),
  });

let laptop;

test("a hook can't be sent without its check page having been shown", async ({ owner, world }) => {
  laptop = machine(world, "laptop");
  installHarnesses(laptop, ["claude"]);
  await pairMachine(owner, world, laptop);
  // A hook added outside Atlas (by hand, an agent, a sync).
  commitHook(world, "outside", "A hook added outside Atlas");
  const { cookie, csrf } = await ownerSecrets(owner);
  const as = { cookie: cookie.value, csrf };

  // Straight to /send/start: the hub answers with the page to look at, not a challenge.
  const start = await post(world, "/send/start", { next: "/settings" }, as);
  expect(start.status, start.body).toBe(200);
  const opts = JSON.parse(start.body);
  expect(opts.review).toMatch(/^\/send\/review\/[0-9a-f]+$/);
  expect(opts.challenge, "no challenge before the page is shown").toBeUndefined();
  // Asking to sign that pending update without loading its page gets no challenge either:
  // the hub points back to the page.
  const pending = opts.review.split("/").pop();
  const early = JSON.parse((await post(world, "/send/start", { next: "/settings", pending }, as)).body);
  expect(early.challenge, "nothing to sign before the page is shown").toBeUndefined();
  expect(early.review).toMatch(/^\/send\/review\//);

  // The owner's way: the page lists the hook with its script, then one tap sends.
  manualReview(owner);
  try {
    await owner.goto("/settings");
    await owner.locator("#updates").getByRole("button", { name: "Send now" }).click();
    await owner.waitForURL(/\/send\/review\//);
    await expect(owner.locator("li", { hasText: "outside" })).toContainText("exit 0");
    await shot(owner, "check before sending lists the new hook");
    await owner.locator("section[data-pending] [data-action='send']").click();
    await owner.waitForURL(/flash=sent/);
  } finally {
    manualReview(owner, false);
  }
  const run = await atlasd(laptop, ["run"]);
  expect(run.code, run.out).toBe(0);
  expect(laptop.read(".claude/settings.json")).toContain("# atlas:outside");
});

test("a hook pushed after the page was opened is not in what machines receive", async ({ owner, world }) => {
  commitHook(world, "shown", "A hook the owner will look at");
  manualReview(owner);
  try {
    await owner.goto("/settings");
    await owner.locator("#updates").getByRole("button", { name: "Send now" }).click();
    await owner.waitForURL(/\/send\/review\//);
    await expect(owner.locator("li", { hasText: "shown" })).toBeVisible();
    await expect(owner.locator("li", { hasText: "late" })).toHaveCount(0);
    await shot(owner, "the page shows only the hook that was there");

    // While the owner reads, something writes another hook to the knowledge repo.
    commitHook(world, "late", "A hook written while the page was open");
    await owner.locator("section[data-pending] [data-action='send']").click();
    await owner.waitForURL(/flash=sent/);
  } finally {
    manualReview(owner, false);
  }
  const run = await atlasd(laptop, ["run"]);
  test.info().attach("atlasd run", { body: run.out, contentType: "text/plain" });
  expect(run.code, run.out).toBe(0);
  const settings = laptop.read(".claude/settings.json");
  expect(settings).toContain("# atlas:shown");
  expect(settings, "what was signed is what the page showed").not.toContain("# atlas:late");

  // The late hook waits for its own look.
  manualReview(owner);
  try {
    await owner.goto("/settings");
    await owner.locator("#updates").getByRole("button", { name: "Send now" }).click();
    await owner.waitForURL(/\/send\/review\//);
    await expect(owner.locator("li", { hasText: "late" })).toBeVisible();
    await shot(owner, "the late hook gets its own check");
  } finally {
    manualReview(owner, false);
  }
});
