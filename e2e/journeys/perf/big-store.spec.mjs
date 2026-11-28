// Performance with a big store (~300 skills, ~300 memory notes, 5 projects): time to
// first render and load of every screen, Save → commit, Send (start → finish) and
// `atlasd run` on a paired machine. Results go to <run>/perf.json and are compared with
// e2e/perf-baseline.json: a number fails only when it is over twice its baseline AND over
// an absolute floor, so noise on a fast machine can't fail it. Re-record the baseline with
// ATLAS_PERF_RECORD=1 after an intended change, and say why in the commit.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { RUN_DIR, git, hubCli, machine } from "../../lib/world.mjs";
import { SCREENS, atlasdAsync, bigStore, pairCodeFrom } from "../../lib/screens.mjs";

test.use({ worldOptions: { purpose: "perf", seedWith: bigStore({ skills: 300, notes: 300, projects: 5 }) } });

const BASELINE = path.join(import.meta.dirname, "../../perf-baseline.json");
const FLOOR = { fcp: 1000, load: 1000, save: 2000, send: 3000, atlasd_run: 3000 };
// Pages share one read of the store for 3 s; wait it out so each page reads the store.
const CACHE_MS = 3100;

/** Merges this project's numbers into a JSON file shared by both projects. */
function merge(file, project, numbers) {
  const all = fs.existsSync(file) ? JSON.parse(fs.readFileSync(file, "utf8")) : {};
  all[project] = numbers;
  fs.writeFileSync(file, JSON.stringify(all, null, 2) + "\n");
}

test("a big store stays quick to read, save, send and apply", async ({ owner, world }, info) => {
  test.setTimeout(240000);
  const project = info.project.name;
  const numbers = { store: { bulkSkills: 300, bulkNotes: 300, projects: 5 }, screens: {} };

  // Every screen, cold: Navigation Timing and the first contentful paint.
  for (const screen of SCREENS) {
    await owner.waitForTimeout(CACHE_MS);
    const res = await owner.goto(screen.path);
    expect(res.status(), screen.path).toBe(200);
    await owner.waitForLoadState("load");
    numbers.screens[screen.path] = await owner.evaluate(() => {
      const n = performance.getEntriesByType("navigation")[0];
      const fcp = performance.getEntriesByName("first-contentful-paint")[0];
      const ms = (v) => Math.round(v);
      return { ttfb: ms(n.responseStart), fcp: ms(fcp ? fcp.startTime : n.domContentLoadedEventEnd), load: ms(n.loadEventEnd) };
    });
  }
  await owner.goto("/skills");
  await expect(owner.locator(".row-name", { hasText: "bulk-skill-299" })).toHaveCount(1);
  await shot(owner, "skills with the bulk rows");

  // Save alone: switch off "send when I save" first, so Send is timed on its own.
  await owner.goto("/settings");
  const sendOnSave = owner.getByRole("checkbox", { name: /Send changes to my machines when I save/ });
  if (await sendOnSave.isChecked()) await sendOnSave.uncheck({ force: true });
  await owner.locator("form.setting-row", { has: sendOnSave }).getByRole("button", { name: "Save" }).click();
  await expect(sendOnSave).not.toBeChecked();

  const before = git(world, world.knowledge, "rev-parse", "HEAD");
  await owner.goto("/skills/private/bulk-skill-150/edit");
  const text = owner.getByLabel("File text");
  await text.fill((await text.inputValue()) + "\nTimed save.\n");
  let t0 = Date.now();
  await owner.getByRole("button", { name: "Save" }).click();
  await owner.waitForURL((u) => !u.pathname.endsWith("/edit"));
  numbers.save = Date.now() - t0;
  await shot(owner, "saved");
  expect(git(world, world.knowledge, "rev-parse", "HEAD")).not.toBe(before);
  expect(git(world, world.knowledge, "show", "HEAD:skills/bulk-skill-150/SKILL.md")).toContain("Timed save.");

  // Send: one passkey tap builds and signs the update.
  await owner.goto("/");
  t0 = Date.now();
  await owner.getByRole("button", { name: "Send", exact: true }).click();
  await owner.waitForURL(/sent=1/, { timeout: 60000 });
  numbers.send = Date.now() - t0;
  await shot(owner, "sent");

  // atlasd on a machine with Claude Code: pair with a fresh code, then time one run.
  const m = machine(world, "laptop");
  fs.mkdirSync(path.join(m.home, ".claude"), { recursive: true });
  const paired = await atlasdAsync(m, "pair", world.url, pairCodeFrom(hubCli(world, "pair-code")), "--machine", "laptop");
  expect(paired.code, paired.out).toBe(0);
  const ran = await atlasdAsync(m, "run");
  numbers.atlasd_run = ran.ms;
  expect(ran.code, ran.out).toBe(0);
  fs.writeFileSync(info.outputPath("atlasd-run.txt"), ran.out);
  const skills = fs.readdirSync(path.join(m.home, ".claude/skills")).filter((n) => n.startsWith("bulk-skill-"));
  expect(skills.length, "atlasd placed every bulk skill").toBe(300);

  merge(path.join(RUN_DIR, "perf.json"), project, numbers);
  await info.attach("perf", { body: JSON.stringify(numbers, null, 2), contentType: "application/json" });

  if (process.env.ATLAS_PERF_RECORD) {
    merge(BASELINE, project, numbers);
    return;
  }
  const base = fs.existsSync(BASELINE) ? JSON.parse(fs.readFileSync(BASELINE, "utf8"))[project] : null;
  expect(base, "a recorded baseline (ATLAS_PERF_RECORD=1)").toBeTruthy();
  const check = (label, now, was, floor) =>
    expect.soft(now > 2 * was && now > floor, `${label}: ${now} ms against a baseline of ${was} ms`).toBe(false);
  for (const [p, t] of Object.entries(numbers.screens)) {
    const b = base.screens[p];
    if (!b) continue;
    check(`${p} first render`, t.fcp, b.fcp, FLOOR.fcp);
    check(`${p} load`, t.load, b.load, FLOOR.load);
  }
  for (const k of ["save", "send", "atlasd_run"]) check(k, numbers[k], base[k], FLOOR[k]);
});
