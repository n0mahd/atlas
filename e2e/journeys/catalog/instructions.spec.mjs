// Instructions: the global AGENTS.md and a project's, their pages, and editing each so
// one commit lands in the repo that owns it.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { git } from "../../lib/world.mjs";
import { SNAPSHOT_TTL_MS, atHead, lastCommit, openEditor, saveText } from "../../lib/catalog.mjs";

const title = (page) => page.locator("header.masthead h1");

test("the list shows the global file, and Showing adds the project's", async ({ owner }) => {
  await owner.goto("/");
  await owner.getByRole("link", { name: "Instructions" }).first().click();
  await expect(title(owner)).toHaveText("Instructions");
  const global = owner.locator("section.section", { has: owner.locator("h2", { hasText: "Global" }) });
  await expect(global.locator(".row-name")).toHaveText(["AGENTS.md"]);
  await expect(global.locator(".row-meta")).toHaveText(/\d+ lines/);
  await shot(owner, "instructions global");

  await owner.getByLabel("Scope").selectOption("shop");
  await expect(owner).toHaveURL(/[?&]scope=shop/);
  const project = owner.locator("section.section", { has: owner.locator("h2", { hasText: "This project" }) });
  await expect(project.locator(".row-name")).toHaveText(["AGENTS.md"]);
  await expect(project.locator("a.row")).toHaveAttribute("href", "/instructions/shop/AGENTS.md");
  await expect(owner.locator(".running")).toContainText("2 files");
  await shot(owner, "instructions for shop");
});

test("each file has its own page", async ({ owner }) => {
  await owner.goto("/instructions");
  await owner.getByRole("link", { name: /AGENTS\.md/ }).click();
  await expect(owner).toHaveURL(/\/instructions\/global\/AGENTS\.md$/);
  await expect(title(owner)).toHaveText("Global AGENTS.md");
  await expect(owner.locator("article.prose h1")).toHaveText("Global agent instructions");
  await expect(owner.locator("dl.facts")).toContainText("~/Knowledge/AGENTS.md");
  await shot(owner, "global AGENTS.md");

  await owner.goto("/instructions?scope=shop");
  await owner.locator('a.row[href="/instructions/shop/AGENTS.md"]').click();
  await expect(title(owner)).toHaveText("shop · AGENTS.md");
  await expect(owner.locator("article.prose")).toContainText("Run the tests before committing.");
  await expect(owner.locator("dl.facts")).toContainText("~/Projects/shop/AGENTS.md");
  await shot(owner, "shop AGENTS.md");

  await owner.getByRole("link", { name: "All instructions" }).click();
  await expect(owner).toHaveURL(/\/instructions$/);

  expect((await owner.request.get("/instructions/no-such-project/AGENTS.md")).status()).toBe(404);
  expect((await owner.request.get("/instructions/global/CLAUDE.md")).status()).toBe(404);
});

test("saving the global file commits it to Knowledge and sends it", async ({ owner, world }) => {
  await owner.goto("/instructions/global/AGENTS.md");
  const editor = await openEditor(owner, "Global AGENTS.md");
  await expect(editor).toHaveValue(/# Global agent instructions/);
  await expect(owner.locator(".editor-bar")).toContainText("~/Knowledge/AGENTS.md");
  await shot(owner, "editing global AGENTS.md");

  const text = "# Global agent instructions\n\nAlways use the simplest solution that works.\n\nRead the strategy first.\n";
  await saveText(owner, text);
  await expect(owner).toHaveURL(/\/instructions\/global\/AGENTS\.md\?saved=[0-9a-f]+.*sent=1/);
  await expect(owner.locator(".flash")).toHaveText("Saved · on your machines within a minute");
  await expect(owner.locator("article.prose")).toContainText("Read the strategy first.");
  await shot(owner, "global AGENTS.md saved");

  const commit = lastCommit(world, world.knowledge);
  expect(commit.subject).toBe("Edit global AGENTS.md in Atlas");
  expect(commit.files).toEqual(["AGENTS.md"]);
  expect(atHead(world, world.knowledge, "AGENTS.md")).toContain("Read the strategy first.");
  expect(git(world, path.join(world.remotes, "Knowledge.git"), "rev-parse", "main")).toBe(commit.sha);
});

test("saving a project's file commits it to the project's main", async ({ owner, world }) => {
  const shop = path.join(world.projectsDir, "shop");
  await owner.goto("/instructions/shop/AGENTS.md");
  await openEditor(owner, "shop · AGENTS.md");
  await expect(owner.locator(".editor-note")).toHaveText("Saving commits to shop's main branch.");
  await saveText(owner, "# Shop\n\nRun the tests before committing.\n\nNever touch the payments table.\n");
  const commit = lastCommit(world, shop);
  await expect(owner.locator(".flash")).toHaveText(`Saved and committed as ${commit.sha.slice(0, 10)}.`);
  await expect(owner.locator("article.prose")).toContainText("Never touch the payments table.");
  await shot(owner, "shop AGENTS.md saved");

  expect(commit.subject).toBe("Edit shop AGENTS.md in Atlas");
  expect(commit.files).toEqual(["AGENTS.md"]);
  expect(atHead(world, shop, "AGENTS.md")).toContain("Never touch the payments table.");
});

test("the list says which agents don't use the global file", async ({ owner, world }) => {
  // On the hub machine, Claude Code reads the store's AGENTS.md through a link; Codex is
  // installed but has no instructions of its own.
  const claude = path.join(world.home, ".claude");
  const codex = path.join(world.home, ".codex");
  fs.mkdirSync(claude, { recursive: true });
  fs.mkdirSync(codex, { recursive: true });
  fs.symlinkSync(path.join(world.knowledge, "AGENTS.md"), path.join(claude, "CLAUDE.md"));
  try {
    await owner.waitForTimeout(SNAPSHOT_TTL_MS);
    await owner.goto("/instructions/global/AGENTS.md");
    await expect(owner.getByRole("img", { name: /^Claude: linked from ~\/\.claude\/CLAUDE\.md$/ })).toBeVisible();
    await shot(owner, "agents reading the global file");

    await owner.goto("/instructions");
    const row = owner.locator('a.row[href="/instructions/global/AGENTS.md"]');
    await shot(owner, "Codex doesn't read it");
    await expect(row.locator(".row-desc")).toHaveText("Not used by Codex");

    // With Codex reading it too, nothing needs flagging.
    fs.symlinkSync(path.join(world.knowledge, "AGENTS.md"), path.join(codex, "AGENTS.md"));
    await owner.waitForTimeout(SNAPSHOT_TTL_MS);
    await owner.reload();
    await shot(owner, "every agent reads it");
    await expect(row.locator(".row-desc")).toHaveCount(0);
  } finally {
    fs.rmSync(claude, { recursive: true, force: true });
    fs.rmSync(codex, { recursive: true, force: true });
    await owner.waitForTimeout(SNAPSHOT_TTL_MS);
  }
});
