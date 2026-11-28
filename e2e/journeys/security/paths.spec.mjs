// Paths: no URL reaches a file outside the data a route owns, links in the store don't
// leak hidden or outside files, and a hook whose `run` climbs out of its folder is flagged
// on the hub and never installed on a machine.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import {
  SENTINEL,
  atlasd,
  form,
  ownerSecrets,
  pairCodeFromUi,
  pairedMachine,
  request,
  seedHostile,
  sendNow,
} from "../../lib/security.mjs";

test.use({ worldOptions: { seedWith: seedHostile } });
test.describe.configure({ mode: "serial" });

let machineLaptop;

const leaks = [SENTINEL.homeEnv, SENTINEL.repoEnv, SENTINEL.outside, "[workspace]", "allowed_logins", "[core]", "root:x:0"];

function expectNoLeak(body, what) {
  for (const l of leaks) expect(body, `${what} leaks ${l}`).not.toContain(l);
}

const traversals = [
  "/static/..%2f..%2f..%2fCargo.toml",
  "/static/../../Cargo.toml",
  "/static/%2e%2e/%2e%2e/Cargo.toml",
  "/static/fonts/..%2f..%2fsrc%2flib.rs",
  "/static/%2fetc%2fpasswd",
  "/skills/private/..%2f..%2f.env",
  "/skills/private/..%252f..%252f.env",
  "/skills/..%2f..%2f..%2f.env/x",
  "/skills/private/%2e%2e/edit",
  "/skills/private/..%2f..%2foutside",
  "/skills/private/..%2f..%2f.env/edit",
  "/memory/..%2f.env",
  "/memory/..%2f..%2f.env",
  "/memory/%2e%2e%2f.env/edit",
  "/memory/leak",
  "/instructions/global/..%2f.env",
  "/instructions/global/.env",
  "/instructions/global/.git%2fconfig",
  "/instructions/global/..%2f..%2f.env/edit",
  "/instructions/..%2f..%2f..%2f/AGENTS.md",
  "/instructions/shop/..%2f..%2f..%2f.env",
  "/instructions/%2fetc/passwd",
  "/install/..%2f..%2fhub.toml",
  "/install/../../hub.toml",
  "/setup/..%2f..%2fhub.toml",
];

test("traversal in any route's path is refused and reads nothing", async ({ owner, world }) => {
  // Attacker: someone with the owner's session probing for files beyond the store.
  const { cookie } = await ownerSecrets(owner);
  for (const p of traversals) {
    const res = await request(world, { path: p, headers: { cookie: `__Host-atlas=${cookie.value}` } });
    expect([400, 404], `${p} → ${res.status}`).toContain(res.status);
    expectNoLeak(res.body, p);
  }
});

test("a save to a traversal path writes nothing", async ({ owner, world }) => {
  const { cookie, csrf } = await ownerSecrets(owner);
  const env = fs.readFileSync(path.join(world.home, ".env"), "utf8");
  for (const p of ["/memory/..%2f..%2f.env/edit", "/instructions/global/..%2f..%2f.env/edit", "/skills/private/..%2f..%2f.env/edit", "/instructions/global/.env/edit"]) {
    const res = await request(world, {
      method: "POST",
      path: p,
      headers: { cookie: `__Host-atlas=${cookie.value}`, origin: world.url, "content-type": "application/x-www-form-urlencoded" },
      body: form({ csrf, base: "x", text: "OVERWRITTEN" }),
    });
    expect([400, 404], `${p} → ${res.status}`).toContain(res.status);
  }
  expect(fs.readFileSync(path.join(world.home, ".env"), "utf8")).toBe(env);
  expect(fs.readFileSync(path.join(world.knowledge, ".env"), "utf8")).not.toContain("OVERWRITTEN");
});

test("links to hidden files or out of the repo are not followed, and are flagged", async ({ owner, world }) => {
  await owner.goto("/skills");
  await expect(owner.locator("main")).toContainText("A skill kept in docs");
  await expect(owner.locator("main")).not.toContainText("Outside the repo");
  await expect(owner.locator("main")).not.toContainText("dotenv");
  await shot(owner, "skills list without the linked-out skills");
  for (const p of ["/skills/private/outside", "/skills/private/dotenv", "/memory/leak"]) {
    const res = await request(world, { path: p });
    expect(res.status, p).toBe(404);
  }
  // A link to a plain file inside the repo is followed (the debt register's rule).
  await owner.goto("/skills/private/kept");
  await expect(owner.locator("main")).toContainText("Kept in docs.");
  await owner.goto("/");
  const needs = owner.locator("section", { has: owner.getByRole("heading", { name: "Needs a look" }) });
  await expect(needs).toContainText("~/Knowledge/skills/outside");
  await expect(needs).toContainText("skipped: links to a hidden file");
  await shot(owner, "needs a look lists the links it skipped");
  for (const p of ["/", "/skills", "/memory", "/hooks", "/instructions", "/skills/private/kept"]) expectNoLeak((await request(world, { path: p })).body, p);
});

test("a memory note that links to a hidden file is flagged like a skill", async ({ owner }) => {
  await owner.goto("/");
  await expect(owner.getByText("~/Knowledge/memory/leak.md")).toBeVisible({ timeout: 3000 });
});

test("a hook whose run climbs out of its folder is flagged", async ({ owner }) => {
  await owner.goto("/hooks");
  const row = owner.locator("details.row", { hasText: "escape" });
  await expect(row.locator(".row-flag")).toHaveText("Script missing");
  await row.locator("summary").click();
  await expect(row).toContainText("../../secret");
  await shot(owner, "escaping hook flagged on the hooks page");
  await owner.goto("/");
  await expect(owner.getByText("`run` must name a file in the hook's own folder")).toBeVisible();
  await shot(owner, "escaping hook in needs a look");
});

test("a machine never installs the escaping hook or the linked-out skills", async ({ owner, world }) => {
  await sendNow(owner);
  const m = await pairedMachine(world, "laptop", await pairCodeFromUi(owner));
  const r = await atlasd(m, "run");
  test.info().attach("atlasd run", { body: r.out, contentType: "text/plain" });
  test.info().annotations.push({ type: "atlasd", description: r.out.split("\n").filter((l) => /escape|outside|dotenv|problem/.test(l)).join(" | ") });
  expect(r.out).toMatch(/escape/);
  expect(r.out).toMatch(/must name a file in the hook's folder/);
  const settings = m.exists(".claude/settings.json") ? m.read(".claude/settings.json") : "";
  expect(settings).not.toContain("secret");
  expect(settings).not.toContain("escape");
  expect(m.exists(".claude/skills/deploy")).toBe(true);
  expect(m.link(".claude/skills/outside")).toBeNull();
  machineLaptop = m;
});


test("a machine doesn't install a skill whose SKILL.md links to a hidden file", async () => {
  test.info().annotations.push({
    type: "bug",
    description:
      "atlasd links ~/.claude/skills/dotenv although its SKILL.md is a link to the repo's .env; the hub skips and flags the same link. The agent then reads .env as a skill.",
  });
  const m = machineLaptop;
  const reads = m.exists(".claude/skills/dotenv/SKILL.md") ? m.read(".claude/skills/dotenv/SKILL.md") : "";
  test.info().annotations.push({ type: "evidence", description: `~/.claude/skills/dotenv -> ${m.link(".claude/skills/dotenv")}; SKILL.md holds the .env sentinel: ${reads.includes(SENTINEL.repoEnv)}` });
  expect(m.link(".claude/skills/dotenv")).toBeNull();
  expect(reads).not.toContain(SENTINEL.repoEnv);
});

test("a machine skips a SKILL.md linked out of the repo and names both skips", async () => {
  const m = machineLaptop;
  expect(m.link(".claude/skills/outfile")).toBeNull();
  const status = await atlasd(m, "run");
  test.info().attach("atlasd run (second pass)", { body: status.out, contentType: "text/plain" });
  for (const name of ["dotenv", "outfile"])
    expect(status.out).toMatch(new RegExp(`skill ${name} skipped: its SKILL.md: it is a link to somewhere else`));
});
