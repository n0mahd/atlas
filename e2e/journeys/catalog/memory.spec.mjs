// Memory: the list and its filters, notes for one host, [[wiki]] links between notes,
// and editing a note so one commit lands in Knowledge.

import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { atHead, lastCommit, openEditor, saveText } from "../../lib/catalog.mjs";

const title = (page) => page.locator("header.masthead h1");
const names = (page) => page.locator("#results .row-name");
const note = (name, description, type, host, body) =>
  `---\nname: ${name}\ndescription: ${description}\nmetadata:\n  type: ${type}\n  host: ${host}\n---\n${body}\n`;

test("the list filters by text, host and kind", async ({ owner }) => {
  await owner.goto("/");
  await owner.getByRole("link", { name: "Memory" }).first().click();
  await expect(title(owner)).toHaveText("Memory");
  await expect(owner.locator(".running")).toContainText("3 notes");
  await expect.poll(async () => (await names(owner).allInnerTexts()).sort()).toEqual(["disk", "user-tz", "verify-outcomes"]);
  // A note kept for one machine says which.
  await expect(owner.locator("a.row", { hasText: "disk" }).locator(".row-meta")).toHaveText("laptop");
  await expect(owner.locator("a.row", { hasText: "user-tz" }).locator(".row-kind")).toHaveText("user");
  await shot(owner, "memory list");

  await owner.getByLabel("Filter memory").fill("time zone");
  await expect(names(owner)).toHaveText(["user-tz"]);
  await shot(owner, "filtered by text");
  await owner.getByLabel("Filter memory").fill("");
  await expect(names(owner)).toHaveCount(3);

  await owner.getByLabel("Host").selectOption("laptop");
  await expect(names(owner)).toHaveText(["disk"]);
  await expect(owner).toHaveURL(/[?&]host=laptop/);
  await shot(owner, "laptop notes");

  await owner.getByLabel("Host").selectOption("shared");
  await expect.poll(async () => (await names(owner).allInnerTexts()).sort()).toEqual(["user-tz", "verify-outcomes"]);

  await owner.locator("label.tab", { hasText: "feedback" }).click();
  await expect(names(owner)).toHaveText(["verify-outcomes"]);
  await shot(owner, "shared feedback");

  await owner.getByLabel("Host").selectOption("laptop");
  await expect(owner.locator("#results .empty")).toHaveText("No note matches.");
  await shot(owner, "no note matches");

  await owner.getByLabel("Host").selectOption("all");
  await owner.locator("label.tab", { hasText: "All" }).click();
  await expect(names(owner)).toHaveCount(3);
});

test("a project with no notes says so", async ({ owner }) => {
  await owner.goto("/memory");
  await owner.getByLabel("Scope").selectOption("shop");
  await expect(owner).toHaveURL(/[?&]scope=shop/);
  await expect(owner.locator("#results .empty")).toHaveText("No memory for this project yet.");
  // Host notes belong to the store, so a project has no host picker.
  await expect(owner.getByLabel("Host")).toHaveCount(0);
  await shot(owner, "shop memory");
});

test("wiki links resolve, navigate and show backlinks", async ({ owner }) => {
  await owner.goto("/memory");
  await owner.locator("a.row", { hasText: "user-tz" }).click();
  await expect(title(owner)).toHaveText("user-tz");
  await expect(owner.locator(".lede")).toHaveText("The owner's time zone");
  const facts = owner.locator("dl.facts");
  await expect(facts).toContainText("user");
  await expect(facts).toContainText("all");
  await expect(facts).toContainText("~/Knowledge/memory/user-tz.md");
  const aside = owner.locator("aside");
  await expect(aside.getByRole("link", { name: "verify-outcomes" })).toBeVisible();
  await shot(owner, "user-tz");

  // [[verify-outcomes]] in the text is a link to that note.
  await owner.locator("article.prose").getByRole("link", { name: "verify-outcomes" }).click();
  await expect(owner).toHaveURL(/\/memory\/verify-outcomes$/);
  await expect(title(owner)).toHaveText("verify-outcomes");
  await expect(owner.locator("article.prose strong")).toHaveText("Why:");
  await expect(owner.locator("aside h2", { hasText: "Linked from" })).toBeVisible();
  await shot(owner, "verify-outcomes with backlink");
  await aside.getByRole("link", { name: "user-tz" }).click();
  await expect(owner).toHaveURL(/\/memory\/user-tz$/);

  expect((await owner.request.get("/memory/no-such-note")).status()).toBe(404);
});

test("a host's note shows its host and file", async ({ owner }) => {
  await owner.goto("/memory?host=laptop");
  await owner.locator("a.row", { hasText: "disk" }).click();
  await expect(title(owner)).toHaveText("disk");
  const facts = owner.locator("dl.facts");
  await expect(facts.locator("dt", { hasText: "Host" }).locator("+ dd")).toHaveText("laptop");
  await expect(facts).toContainText("~/Knowledge/memory/hosts/laptop/disk.md");
  await expect(owner.locator("article.prose")).toContainText("One NVMe drive.");
  await shot(owner, "disk note");
});

test("saving a note commits it to Knowledge, and new links show", async ({ owner, world }) => {
  await owner.goto("/memory/verify-outcomes");
  const editor = await openEditor(owner, "verify-outcomes");
  await expect(editor).toHaveValue(/\*\*Why:\*\* status lies\./);
  await expect(owner.locator(".editor-bar")).toContainText("~/Knowledge/memory/verify-outcomes.md");
  await shot(owner, "editing a note");

  await saveText(
    owner,
    note(
      "verify-outcomes",
      "Verify outcomes, not status",
      "feedback",
      "all",
      "Confirm what the user sees before claiming a fix worked. Times are in [[user-tz]]; see also [[nowhere]].\n\n**Why:** status lies.",
    ),
  );
  await expect(owner).toHaveURL(/\/memory\/verify-outcomes\?saved=[0-9a-f]+.*sent=1/);
  await expect(owner.locator(".flash")).toHaveText("Saved · on your machines within a minute");
  const prose = owner.locator("article.prose");
  await expect(prose.getByRole("link", { name: "user-tz" })).toHaveAttribute("href", "/memory/user-tz");
  // A link to a note that doesn't exist stays plain text, and the aside says so.
  await expect(prose).toContainText("[[nowhere]]");
  await expect(prose.getByRole("link", { name: "nowhere" })).toHaveCount(0);
  await expect(owner.locator("aside")).toContainText("nowhere (no page yet)");
  await shot(owner, "note saved with links");

  const commit = lastCommit(world, world.knowledge);
  expect(commit.subject).toBe("Edit memory verify-outcomes in Atlas");
  expect(commit.files).toEqual(["memory/verify-outcomes.md"]);
  expect(atHead(world, world.knowledge, "memory/verify-outcomes.md")).toContain("[[nowhere]]");
});

test("saving a host's note commits it under that host's folder", async ({ owner, world }) => {
  await owner.goto("/memory/disk");
  await openEditor(owner, "disk");
  await saveText(owner, note("disk", "Laptop disk layout", "reference", "laptop", "One NVMe drive, 2 TB."));
  await expect(owner.locator("article.prose")).toContainText("One NVMe drive, 2 TB.");
  await shot(owner, "host note saved");
  const commit = lastCommit(world, world.knowledge);
  expect(commit.subject).toBe("Edit memory disk in Atlas");
  expect(commit.files).toEqual(["memory/hosts/laptop/disk.md"]);
  expect(atHead(world, world.knowledge, path.posix.join("memory/hosts/laptop/disk.md"))).toContain("2 TB");
});
