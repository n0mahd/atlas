// Real harnesses see what atlasd wrote. The owner sends the seeded store from the front
// page, a laptop pairs and runs atlasd, and then each installed harness (Claude Code, Codex,
// OpenCode, Pi, Amp, Muse, Grok Build, Antigravity) is asked offline what it loaded: skills,
// instructions, hooks and subagents. Then a skill is switched off for some agents, and a
// project checkout's own skill is looked for inside the checkout. Each probe's raw output
// is attached, so the report shows what the harness itself said.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { machine, git } from "../../lib/world.mjs";
import {
  HARNESSES,
  atlasd,
  createMarkers,
  neutralDir,
  pair,
  probeAll,
  readsFile,
  sendFromFrontPage,
} from "../../lib/harnesses.mjs";

test.describe.configure({ mode: "serial" });
test.setTimeout(300000);
test.beforeEach(() => {
  test.skip(test.info().project.name === "phone", "Probes ask the harnesses on the machine; the browser's viewport plays no part.");
});

const GLOBAL_SKILLS = ["deploy", "pdf-tools", "review"];
const MARKER = "Always use the simplest solution that works.";
const OFF_FOR = ["claude-code", "codex", "pi"];

let laptop;
let probes;
let scratch; // a cwd outside any repo and the real HOME, for the home-level probes
test.afterAll(() => scratch && fs.rmSync(scratch, { recursive: true, force: true }));
const outside = () => (scratch ??= neutralDir());

/** Attaches each harness's raw answer and returns the installed ones. */
async function evidence(results, label) {
  for (const [name, p] of Object.entries(results)) {
    const { raw, ...summary } = p;
    await test.info().attach(`${name} · ${label}.txt`, {
      body: `${JSON.stringify(summary, null, 2)}\n\n---- what ${name} said\n${raw}`,
      contentType: "text/plain",
    });
    if (!p.installed) test.info().annotations.push({ type: "skip", description: `${name}: ${p.gaps[0]}` });
  }
  return Object.entries(results).filter(([, p]) => p.installed);
}

async function run(m) {
  const r = await atlasd(m, "run");
  await test.info().attach(`atlasd run · ${new Date().toISOString()}.txt`, { body: r.out, contentType: "text/plain" });
  expect(r.code, r.out).toBe(0);
  return r.out;
}

// Its own world: the first journey expects a hub that has never sent anything.
test.use({ worldOptions: { tag: "harnesses" } });

test("the owner sends and a laptop with every harness applies it", async ({ owner, world }) => {
  await owner.goto("/");
  await expect(owner.getByText("Nothing has been sent to your machines yet")).toBeVisible();
  await shot(owner, "front page before Send");
  await sendFromFrontPage(owner);
  await shot(owner, "sent");

  laptop = machine(world, "laptop");
  createMarkers(laptop.home);
  await pair(world, laptop);
  const out = await run(laptop);
  expect(out).toContain("Update 1 is applied");

  await owner.goto("/machines");
  await expect(owner.getByText("Update 1, up to date")).toBeVisible();
  await shot(owner, "machines: laptop up to date");
});

test("every harness lists the global skills", async () => {
  test.skip(!laptop, "the laptop didn't pair");
  probes = await probeAll({ home: laptop.home, cwd: outside(), marker: MARKER, check: GLOBAL_SKILLS });
  const installed = await evidence(probes, "global skills");
  expect(installed.length, "at least one harness is installed").toBeGreaterThan(0);
  for (const [name, p] of installed)
    for (const skill of GLOBAL_SKILLS) {
      expect.soft(p.skills, `${name} lists ${skill}`).toContain(skill);
      // OpenCode, Amp, Muse and Grok Build read both ~/.claude/skills and ~/.agents/skills.
      expect.soft(p.skills.filter((s) => s === skill).length, `${name} lists ${skill} once`).toBe(1);
      expect.soft(p.off, `${name} has ${skill} on`).not.toContain(skill);
    }
});

test("instructions, hooks and subagents are seen where each harness reports them", async () => {
  test.skip(!probes, "no probes ran");
  const store = path.join(laptop.home, ".local/share/atlas/sources/knowledge/AGENTS.md");
  for (const [name, p] of Object.entries(probes).filter(([, p]) => p.installed)) {
    if (p.instructions !== null)
      expect.soft(readsFile(p.instructions, store), `${name} loads the global AGENTS.md (it reported ${JSON.stringify(p.instructions)})`).toBe(true);
    if (p.promptHasMarker !== undefined) expect.soft(p.promptHasMarker, `${name}'s prompt holds the instructions`).toBe(true);
  }
  // Atlas writes hooks for Claude Code, Codex, Grok Build (through Claude's settings, which
  // Grok also runs) and Antigravity; the last only shows hooks by running them.
  const seesNoRm = {
    "claude-code": (h) => h.event === "PreToolUse" && h.matcher === "Bash",
    codex: (h) => h.atlas === "no-rm" && h.event === "preToolUse" && h.matcher === "Bash",
    grok: (h) => h.atlas === "no-rm" && h.event === "pre_tool_use" && h.matcher === "Bash",
  };
  for (const [name, match] of Object.entries(seesNoRm)) {
    const p = probes[name];
    if (!p.installed) continue;
    expect.soft(p.hooks.some(match), `${name} sees the no-rm hook (it reported ${JSON.stringify(p.hooks)})`).toBe(true);
  }
  for (const name of ["claude-code", "grok"]) {
    if (!probes[name].installed) continue;
    expect.soft(probes[name].agents, `${name} lists the reviewer subagent`).toContain("reviewer");
  }
});

test("Claude Code runs the no-rm hook cleanly", async () => {
  test.skip(!probes?.["claude-code"].installed, "Claude Code isn't installed");
  const runs = probes["claude-code"].hooks.filter((h) => h.event === "PreToolUse");
  expect(runs.length).toBeGreaterThan(0);
  for (const h of runs) expect(h.ran, JSON.stringify(h.ran)).toMatchObject({ exit: 0 });
});

test("a skill switched off for some agents is gone for them only", async ({ owner, world }) => {
  test.skip(!laptop, "the laptop didn't pair");
  test.fail(true, "known bug: atlasd ignores atlas/placements.toml");
  test.info().annotations.push({
    type: "bug",
    description:
      "atlasd (crates/atlas-client) never reads atlas/placements.toml or writes any per-agent switch: it links every " +
      "skill into the first folder of each present harness, so a skill switched off for claude-code, codex and pi is " +
      "still listed and enabled by all of them (documents/skills.md, decision 0027).",
  });
  // The hub has no switch for this yet (skills_place::set_agents isn't built), so the owner
  // edits the placements file in Knowledge by hand and commits.
  const file = path.join(world.knowledge, "atlas/placements.toml");
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `[global.pdf-tools]\noff = [${OFF_FOR.map((a) => `"${a}"`).join(", ")}]\n`);
  git(world, world.knowledge, "add", "atlas/placements.toml");
  git(world, world.knowledge, "commit", "-q", "-m", "Switch pdf-tools off for Claude Code, Codex and Pi");

  await owner.goto("/");
  await expect(owner.getByText("1 change isn't on your machines yet")).toBeVisible();
  await shot(owner, "front page: the placements change is waiting");
  await sendFromFrontPage(owner);
  await shot(owner, "placements sent");
  expect(git(world, world.knowledge, "status", "-sb")).not.toContain("ahead");
  const out = await run(laptop);
  expect(out).toContain("Update 2 is applied");

  const after = await probeAll({ home: laptop.home, cwd: outside(), marker: MARKER, check: ["pdf-tools"] });
  const installed = await evidence(after, "pdf-tools switched off");
  for (const [name, p] of installed) {
    if (name === "antigravity") continue; // no known switch to check (Gaps)
    const offered = p.skills.includes("pdf-tools") && !p.off.includes("pdf-tools");
    expect.soft(offered, `${name} ${OFF_FOR.includes(name) ? "no longer offers" : "still offers"} pdf-tools`).toBe(!OFF_FOR.includes(name));
    for (const other of ["deploy", "review"]) expect.soft(p.skills, `${name} still lists ${other}`).toContain(other);
  }
});

test("a project checkout's own skill shows inside it", async ({ world }) => {
  test.skip(!laptop, "the laptop didn't pair");
  test.fail(true, "known bug: atlasd makes no project links, so Claude Code doesn't see project skills");
  test.info().annotations.push({
    type: "bug",
    description:
      "Updates name no projects (atlas-web release::build leaves them out) and atlasd never looks for checkouts, so " +
      "it doesn't link a project's .agents/skills into .claude/skills: Claude Code doesn't list shop's stock skill " +
      "(documents/skills.md 'Projects on each machine', decision 0027). Harnesses that read .agents/skills themselves see it.",
  });
  const shop = path.join(laptop.home, "Projects/shop");
  fs.mkdirSync(path.dirname(shop), { recursive: true });
  git(world, world.root, "clone", "-q", path.join(world.remotes, "shop.git"), shop);
  await run(laptop);

  const inShop = await probeAll({ home: laptop.home, cwd: shop, marker: MARKER, trust: true });
  const installed = await evidence(inShop, "in ~/Projects/shop");
  for (const [name, p] of installed) {
    expect.soft(p.skills, `${name} lists shop's stock skill in the checkout`).toContain("stock");
    for (const skill of GLOBAL_SKILLS.filter((s) => s !== "pdf-tools"))
      expect.soft(p.skills, `${name} still lists ${skill} there`).toContain(skill);
  }
  // The links atlasd makes in a checkout stay out of the project's git.
  const exclude = fs.readFileSync(path.join(shop, ".git/info/exclude"), "utf8");
  expect.soft(exclude, "atlasd's .claude/skills links are git-excluded").toMatch(/^\/?\.claude\/skills\/?$/m);
  expect.soft(git(world, shop, "status", "--porcelain"), "the checkout stays clean").toBe("");
});

test("every harness atlasd knows is probed", () => {
  // Keeps this journey in step with crates/atlas-harness: a new layout needs a probe here.
  const src = fs.readFileSync(path.resolve(import.meta.dirname, "../../../crates/atlas-harness/src/lib.rs"), "utf8");
  const markers = [...src.matchAll(/marker: "([^"]+)"/g)].map((m) => m[1]).filter((m) => m !== ".t3" && m !== ".config/herdr");
  expect(markers.sort()).toEqual(Object.values(HARNESSES).map((h) => h.marker).sort());
});
