// Skills: the list, its filter and pickers, each kind of skill's page, and editing a skill
// so that exactly one commit lands in the repo that owns it.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot, allowConsole } from "../../lib/owner.mjs";
import { git } from "../../lib/world.mjs";
import {
  SNAPSHOT_TTL_MS,
  atHead,
  handCommit,
  handEdit,
  lastCommit,
  openEditor,
  saveText,
} from "../../lib/catalog.mjs";

const skillMd = (name, description, body) => `---\nname: ${name}\ndescription: ${description}\n---\n# ${name}\n\n${body}\n`;
const REFUSED = /status of 409 \(Conflict\)/;
const title = (page) => page.locator("header.masthead h1");
const rowNames = (page, section) => page.locator(`section#${section} .row-name`);

test("the list shows every global skill, and the filter narrows it", async ({ owner }) => {
  await owner.goto("/");
  await owner.getByRole("link", { name: "Skills" }).first().click();
  await expect(owner.getByRole("heading", { level: 1, name: "Skills" })).toBeVisible();
  await expect(owner.locator(".running")).toContainText("3 skills");
  await expect(rowNames(owner, "global")).toHaveCount(3);
  expect((await rowNames(owner, "global").allInnerTexts()).sort()).toEqual(["deploy", "pdf-tools", "review"]);
  await shot(owner, "skills list");

  // A row opens inline to show where the skill lives and a link to read it.
  const deploy = owner.locator("details.row", { hasText: "Ship the app to production" });
  await deploy.locator("summary").click();
  await expect(deploy.locator(".drawer")).toContainText("~/Knowledge/skills/deploy/SKILL.md · 1 more files");
  await expect(deploy.getByRole("link", { name: "Read the skill" })).toHaveAttribute("href", "/skills/private/deploy");
  await shot(owner, "deploy row open");

  const filter = owner.getByLabel("Filter skills");
  await filter.fill("pdf");
  await expect(owner).toHaveURL(/[?&]q=pdf/);
  await expect(rowNames(owner, "global")).toHaveText(["pdf-tools"]);
  await shot(owner, "filtered to pdf");

  // The filter matches descriptions too.
  await filter.fill("correctness");
  await expect(rowNames(owner, "global")).toHaveText(["review"]);

  await filter.fill("no-such-skill");
  await expect(owner.locator("section#global .empty")).toHaveText("No skill here matches.");
  await shot(owner, "filter matches nothing");

  await filter.fill("");
  await expect(rowNames(owner, "global")).toHaveCount(3);
});

test("Showing picks a project's skills; the machine picker lists unpaired machines", async ({ owner }) => {
  await owner.goto("/skills");
  const machine = owner.getByLabel("Machine");
  await expect(machine.locator("option:checked")).toHaveText("verify-hub");
  await expect(machine.locator("option", { hasText: "laptop (not paired)" })).toBeDisabled();

  await owner.getByLabel("Scope").selectOption("shop");
  await expect(owner).toHaveURL(/[?&]scope=shop/);
  await expect(owner.locator("section#project h2")).toHaveText("This project");
  await expect(rowNames(owner, "project")).toHaveText(["stock"]);
  // Global skills still show under the project, numbered after it.
  await expect(owner.locator("section#global .num")).toHaveText("II");
  await expect(rowNames(owner, "global")).toHaveCount(3);
  await expect(owner.locator(".running")).toContainText("4 skills");
  // Scoped sections keep the project as the owner moves between them.
  await expect(owner.getByRole("link", { name: "Instructions" }).first()).toHaveAttribute("href", "/instructions?scope=shop");
  await shot(owner, "skills for shop");

  const stock = owner.locator("details.row", { hasText: "Count stock" });
  await stock.locator("summary").click();
  await expect(stock.locator(".drawer")).toContainText("~/Projects/shop/.agents/skills/stock/SKILL.md");

  // An unknown project falls back to Global.
  await owner.goto("/skills?scope=no-such-project");
  await expect(owner.getByLabel("Scope").locator("option:checked")).toHaveText("Global");
  await expect(owner.locator("section#project")).toHaveCount(0);

  await owner.getByLabel("Scope").selectOption("global");
  await expect(owner.locator("section#project")).toHaveCount(0);
});

test("each kind of skill has its own page", async ({ owner }) => {
  await owner.goto("/skills");
  await owner.locator("details.row", { hasText: "Ship the app" }).locator("summary").click();
  await owner.getByRole("link", { name: "Read the skill" }).first().click();
  await expect(owner).toHaveURL(/\/skills\/private\/deploy$/);
  await expect(title(owner)).toHaveText("deploy");
  await expect(owner.locator(".lede")).toHaveText("Ship the app to production");
  await expect(owner.locator("article.prose")).toContainText("Run the deploy script.");
  const facts = owner.locator("dl.facts");
  await expect(facts).toContainText("Global, private");
  await expect(facts).toContainText("~/Knowledge/skills/deploy/SKILL.md");
  await expect(owner.locator("pre.ink-block")).toContainText("scripts/run.sh");
  await shot(owner, "private skill page");

  await owner.goto("/skills/public/review");
  await expect(facts).toContainText("Global, public");
  await expect(facts).toContainText("~/Skills/review/SKILL.md");
  await shot(owner, "public skill page");

  await owner.goto("/skills/shop/stock");
  await expect(facts).toContainText("The shop repo");
  await expect(facts).toContainText("~/Projects/shop/.agents/skills/stock/SKILL.md");
  await shot(owner, "project skill page");

  await owner.getByRole("link", { name: "All skills" }).click();
  await expect(owner).toHaveURL(/\/skills$/);

  const missing = await owner.request.get("/skills/private/no-such-skill");
  expect(missing.status()).toBe(404);
  expect(await missing.text()).toContain("Not in the atlas.");
});

test("saving a private skill commits it to Knowledge and sends it", async ({ owner, world }) => {
  const rel = "skills/deploy/SKILL.md";
  const before = lastCommit(world, world.knowledge).sha;
  // The owner has an unrelated hand edit open in the same repo: Atlas must leave it alone.
  handEdit(path.join(world.knowledge, "AGENTS.md"), "# Global agent instructions\n\nWork in progress.\n");

  await owner.goto("/skills/private/deploy");
  const editor = await openEditor(owner, "deploy");
  await expect(editor).toHaveValue(/Run the deploy script\./);
  await expect(owner.locator(".editor-bar")).toContainText("~/Knowledge/skills/deploy/SKILL.md");
  await expect(owner.locator(".editor-note")).toHaveText("Saving commits to the knowledge repo and sends it to your machines.");
  await shot(owner, "editing deploy");

  const text = skillMd("deploy", "Ship the app to production", "Run the deploy script, then watch the logs for five minutes.");
  await saveText(owner, text);
  await expect(owner).toHaveURL(/\/skills\/private\/deploy\?saved=[0-9a-f]+.*sent=1/);
  await expect(owner.locator(".flash")).toHaveText("Saved · on your machines within a minute");
  await expect(owner.locator("article.prose")).toContainText("watch the logs for five minutes");
  await shot(owner, "deploy saved and sent");

  const commit = lastCommit(world, world.knowledge);
  expect(commit.sha).not.toBe(before);
  expect(commit.subject).toBe("Edit skill deploy in Atlas");
  expect(commit.files).toEqual([rel]);
  expect(atHead(world, world.knowledge, rel)).toContain("watch the logs for five minutes");
  // Sending pushed Knowledge to its remote.
  expect(git(world, world.knowledge, "rev-parse", "origin/main")).toBe(commit.sha);
  expect(git(world, path.join(world.remotes, "Knowledge.git"), "rev-parse", "main")).toBe(commit.sha);
  // The owner's hand edit is still there, uncommitted.
  expect(git(world, world.knowledge, "status", "--porcelain", "AGENTS.md")).toBe("M AGENTS.md");
  git(world, world.knowledge, "checkout", "--", "AGENTS.md");
});

test("saving a public skill commits to the Skills repo and never pushes it", async ({ owner, world }) => {
  const remoteBefore = git(world, path.join(world.remotes, "Skills.git"), "rev-parse", "main");
  await owner.goto("/skills/public/review");
  await openEditor(owner, "review");
  await expect(owner.locator(".editor-note")).toContainText("Atlas never pushes it");
  await saveText(owner, skillMd("review", "Review code for correctness", "Read every changed line twice."));
  await expect(owner).toHaveURL(/\/skills\/public\/review\?saved=[0-9a-f]+&public=1/);
  await expect(owner.locator(".flash")).toHaveText("Saved. Public skills reach your machines after you push them");
  await expect(owner.locator("article.prose")).toContainText("Read every changed line twice.");
  await shot(owner, "public skill saved");

  const commit = lastCommit(world, world.publicSkills);
  expect(commit.subject).toBe("Edit skill review in Atlas");
  expect(commit.files).toEqual(["review/SKILL.md"]);
  expect(atHead(world, world.publicSkills, "review/SKILL.md")).toContain("Read every changed line twice.");
  expect(git(world, path.join(world.remotes, "Skills.git"), "rev-parse", "main")).toBe(remoteBefore);
});

test("saving a project skill commits to the project's main", async ({ owner, world }) => {
  const shop = path.join(world.projectsDir, "shop");
  await owner.goto("/skills/shop/stock");
  await openEditor(owner, "stock");
  await expect(owner.locator(".editor-note")).toHaveText("Saving commits to shop's main branch.");
  await saveText(owner, skillMd("stock", "Count stock in the shop database", "Count twice; the database lags."));
  const sha = lastCommit(world, shop).sha;
  await expect(owner.locator(".flash")).toHaveText(`Saved and committed as ${sha.slice(0, 10)}.`);
  await expect(owner.locator("article.prose")).toContainText("Count twice; the database lags.");
  await shot(owner, "project skill saved");

  const commit = lastCommit(world, shop);
  expect(commit.subject).toBe("Edit skill stock in Atlas");
  expect(commit.files).toEqual([".agents/skills/stock/SKILL.md"]);
  expect(git(world, shop, "branch", "--show-current")).toBe("main");
  // Pushing after save is off by default.
  expect(git(world, path.join(world.remotes, "shop.git"), "rev-parse", "main")).not.toBe(commit.sha);
});

test("saving the same text commits nothing", async ({ owner, world }) => {
  const before = lastCommit(world, world.knowledge).sha;
  await owner.goto("/skills/private/pdf-tools");
  const editor = await openEditor(owner, "pdf-tools");
  await saveText(owner, await editor.inputValue());
  await expect(owner.locator(".flash")).toHaveText("Nothing changed, so nothing was committed.");
  await shot(owner, "unchanged save");
  expect(lastCommit(world, world.knowledge).sha).toBe(before);

  // Cancel goes back without saving.
  await openEditor(owner, "pdf-tools");
  await owner.getByRole("link", { name: "Cancel" }).click();
  await expect(owner).toHaveURL(/\/skills\/private\/pdf-tools$/);
});

test("a save refuses a file with uncommitted hand edits", async ({ owner, world }) => {
  const file = path.join(world.knowledge, "skills/pdf-tools/SKILL.md");
  const before = lastCommit(world, world.knowledge).sha;
  handEdit(file, skillMd("pdf-tools", "Read and merge PDF files", "Hand edit in progress."));
  allowConsole(owner, REFUSED);

  await owner.goto("/skills/private/pdf-tools");
  await openEditor(owner, "pdf-tools");
  await saveText(owner, skillMd("pdf-tools", "Read and merge PDF files", "Edited in Atlas."));
  await expect(owner.getByRole("alert")).toHaveText(
    "This file has uncommitted changes made outside Atlas. Commit or discard them first.",
  );
  // The owner's text is still in the editor, so nothing typed is lost.
  await expect(owner.getByLabel("File text")).toHaveValue(/Edited in Atlas\./);
  await shot(owner, "refused: uncommitted changes");

  expect(lastCommit(world, world.knowledge).sha).toBe(before);
  expect(handText(file)).toContain("Hand edit in progress.");
  git(world, world.knowledge, "checkout", "--", "skills/pdf-tools/SKILL.md");
});

test("a save refuses a file that changed since the editor opened", async ({ owner, world }) => {
  const file = path.join(world.knowledge, "skills/pdf-tools/SKILL.md");
  allowConsole(owner, REFUSED);
  await owner.goto("/skills/private/pdf-tools");
  await openEditor(owner, "pdf-tools");
  await shot(owner, "editor open");

  // Meanwhile the owner edits and commits the file by hand elsewhere.
  handEdit(file, skillMd("pdf-tools", "Read and merge PDF files", "Changed by hand and committed."));
  handCommit(world, world.knowledge, "Hand edit pdf-tools", "skills/pdf-tools/SKILL.md");
  const handSha = lastCommit(world, world.knowledge).sha;

  await saveText(owner, skillMd("pdf-tools", "Read and merge PDF files", "Stale copy edited in Atlas."));
  await expect(owner.getByRole("alert")).toHaveText(
    "The file changed since you opened it. Your text is below; copy what you need and reload.",
  );
  await expect(owner.getByLabel("File text")).toHaveValue(/Stale copy edited in Atlas\./);
  await shot(owner, "refused: changed meanwhile");

  expect(lastCommit(world, world.knowledge).sha).toBe(handSha);
  expect(handText(file)).toContain("Changed by hand and committed.");

  // Reloading the editor picks up the hand edit.
  await owner.goto("/skills/private/pdf-tools/edit");
  await expect(owner.getByLabel("File text")).toHaveValue(/Changed by hand and committed\./);
});

test("markdown in a skill renders inert: no script, handler or javascript: link", async ({ owner, world }) => {
  // A skill installed from somewhere else, committed by hand: Atlas must not run it.
  const rel = "skills/untrusted/SKILL.md";
  handEdit(
    path.join(world.knowledge, rel),
    skillMd(
      "untrusted",
      "A skill from the internet",
      [
        "Safe text before.",
        "",
        "<script>alert('script')</script>",
        "",
        '<img src="/static/favicon.png" onerror="alert(\'onerror\')" alt="pic">',
        "",
        "[click me](javascript:alert('link'))",
        "",
        '<a href="javascript:alert(\'raw\')" onclick="alert(\'click\')">raw link</a>',
        "",
        "Safe text after.",
      ].join("\n"),
    ),
  );
  handCommit(world, world.knowledge, "Add untrusted skill", rel);

  const dialogs = [];
  owner.on("dialog", (d) => {
    dialogs.push(d.message());
    d.dismiss();
  });
  await owner.waitForTimeout(SNAPSHOT_TTL_MS);
  await owner.goto("/skills/private/untrusted");
  const prose = owner.locator("article.prose");
  await expect(prose).toContainText("Safe text before.");
  await expect(prose).toContainText("Safe text after.");
  await expect(prose.locator("script")).toHaveCount(0);
  await expect(prose.locator("[onerror], [onclick]")).toHaveCount(0);
  await expect(prose.locator('a[href^="javascript:" i]')).toHaveCount(0);
  await expect(prose.locator("img")).toHaveCount(1);
  await shot(owner, "untrusted skill rendered");

  // Clicking what is left of the links does nothing harmful.
  for (const name of ["click me", "raw link"]) {
    const link = prose.getByText(name);
    if (await link.count()) await link.first().click();
  }
  await owner.waitForTimeout(300);
  expect(dialogs).toEqual([]);

  // The editor shows the source as text, not as markup.
  await owner.goto("/skills/private/untrusted/edit");
  await expect(owner.getByLabel("File text")).toHaveValue(/<script>alert\('script'\)<\/script>/);
  await expect(owner.locator("form.editor script")).toHaveCount(0);
  await owner.waitForTimeout(300);
  expect(dialogs).toEqual([]);
  await shot(owner, "untrusted skill in the editor");
});

const handText = (file) => fs.readFileSync(file, "utf8");
