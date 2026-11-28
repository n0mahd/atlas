// Hooks: read-only. Atlas's own hooks with their manifest facts, and hooks an agent on the
// hub machine has of its own, listed as outside Atlas.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { SNAPSHOT_TTL_MS } from "../../lib/catalog.mjs";

const section = (page, name) => page.locator("section.section", { has: page.locator("h2", { hasText: name }) });

test("hooks in Atlas show their manifest; none outside", async ({ owner }) => {
  await owner.goto("/");
  await owner.getByRole("link", { name: "Hooks" }).first().click();
  await expect(owner.locator("header.masthead h1")).toHaveText("Hooks");
  await expect(owner.locator(".running")).toContainText("1 in Atlas · 0 outside");
  const atlas = section(owner, "In Atlas");
  const row = atlas.locator("details.row", { hasText: "no-rm" });
  await expect(row.locator(".row-desc")).toHaveText("Block rm -rf");
  await expect(row.locator(".row-meta")).toHaveText("before a tool runs");
  await expect(row.locator(".row-flag")).toHaveCount(0);
  await row.locator("summary").click();
  const facts = row.locator("dl.facts");
  await expect(facts).toContainText("Global");
  await expect(facts).toContainText("Bash");
  await expect(facts).toContainText("check.sh · 10s");
  await expect(facts).toContainText("every agent with this event");
  await expect(facts).toContainText("~/Knowledge/hooks/no-rm/hook.toml");
  await expect(section(owner, "Outside Atlas").locator(".empty")).toHaveText(
    "No agent on this machine has hooks of its own.",
  );
  // Hooks have no editor.
  await expect(owner.locator(".edit-button")).toHaveCount(0);
  await shot(owner, "hooks");
});

test("a hand-made agent hook shows outside Atlas", async ({ owner, world }) => {
  const claude = path.join(world.home, ".claude");
  fs.mkdirSync(claude, { recursive: true });
  fs.writeFileSync(
    path.join(claude, "settings.json"),
    JSON.stringify({
      hooks: {
        PreToolUse: [{ matcher: "Bash", hooks: [{ type: "command", command: "~/bin/guard.sh", timeout: 5 }] }],
      },
    }),
  );
  try {
    await owner.waitForTimeout(SNAPSHOT_TTL_MS);
    await owner.goto("/hooks");
    await expect(owner.locator(".running")).toContainText("1 in Atlas · 1 outside");
    const row = section(owner, "Outside Atlas").locator("details.row");
    await expect(row).toHaveCount(1);
    await expect(row.locator(".row-name")).toHaveText("~/bin/guard.sh");
    await expect(row.locator(".row-meta")).toContainText("Bash");
    await expect(row.getByRole("img")).toHaveAttribute("aria-label", /Claude/);
    await row.locator("summary").click();
    await expect(row.locator(".drawer")).toContainText("~/.claude/settings.json");
    await expect(row.locator(".drawer")).toContainText("times out after");
    await shot(owner, "hook outside Atlas");
  } finally {
    fs.rmSync(claude, { recursive: true, force: true });
    await owner.waitForTimeout(SNAPSHOT_TTL_MS);
  }
});
