// Every screen of the hub, for the visual, accessibility and performance journeys, plus
// what changes per run on each (masked in screenshots) and a few small helpers.

import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { addAuthenticator, watchConsole } from "./owner.mjs";
import { BIN, sandboxEnv } from "./world.mjs";

/**
 * The signed-in screens. Detail paths follow from the seed store (`seed.mjs`); `mask`
 * names elements whose text changes per run (dates, ports, codes).
 */
export const SCREENS = [
  { name: "front", path: "/" },
  { name: "skills", path: "/skills" },
  { name: "skill-detail", path: "/skills/private/deploy" },
  { name: "instructions", path: "/instructions" },
  { name: "instruction-detail", path: "/instructions/global/AGENTS.md" },
  { name: "memory", path: "/memory" },
  { name: "memory-detail", path: "/memory/verify-outcomes" },
  { name: "hooks", path: "/hooks" },
  { name: "mcp", path: "/mcp" },
  { name: "machines", path: "/machines" },
  { name: "settings", path: "/settings", mask: [".row-desc.mono"] },
  { name: "edit", path: "/skills/private/deploy/edit" },
];

export const SCHEMES = ["light", "dark"];

/** Locators to mask on a screen. */
export function masks(page, screen) {
  return (screen.mask ?? []).map((sel) => page.locator(sel));
}

/** Waits until the page's web fonts are in, so text metrics don't move between runs. */
export async function settle(page) {
  await page.waitForLoadState("load");
  await page.evaluate(() => document.fonts.ready);
}

/**
 * A second browser context in the same project (device, viewport) with nobody signed in,
 * for sign-in screens; `credentials` (from another authenticator) let it sign in.
 * Returns { context, page, auth, problems }.
 */
export async function strangerPage(browser, world, info, credentials = []) {
  const context = await browser.newContext({ ...info.project.use, baseURL: world.url });
  const page = await context.newPage();
  const problems = watchConsole(page);
  const auth = await addAuthenticator(page, credentials);
  return { context, page, auth, problems };
}

/**
 * Runs atlasd on machine `m` without blocking. `m.atlasd()` uses execFileSync, which
 * stalls this process's event loop, and with it the world's proxy, so any atlasd command
 * that talks to the hub (pair, run) can't get an answer. Resolves to { code, out, ms }.
 */
export function atlasdAsync(m, ...args) {
  return new Promise((resolve) => {
    const t0 = Date.now();
    const p = spawn(path.join(BIN, "atlasd"), args, { env: sandboxEnv(m.home), stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    p.stdout.on("data", (c) => (out += c));
    p.stderr.on("data", (c) => (out += c));
    p.on("close", (code) => resolve({ code, out, ms: Date.now() - t0 }));
  });
}

/** The one-time code inside `atlas-hub pair-code`'s printed install command. */
export function pairCodeFrom(output) {
  const m = output.match(/sh -s -- (\S+)/);
  if (!m) throw new Error(`no pairing code in:\n${output}`);
  return m[1];
}

/** The setup link `atlas-hub setup` prints. */
export function setupLinkFrom(output) {
  const m = output.match(/(https?:\/\/\S+\/setup\/\S+)/);
  if (!m) throw new Error(`no setup link in:\n${output}`);
  return m[1];
}

function commitAll(home, dir, message) {
  const env = sandboxEnv(home);
  const run = (cwd, ...args) => execFileSync("git", args, { cwd, env, stdio: "pipe" });
  run(dir, "add", "-A");
  run(dir, "commit", "-q", "-m", message);
}

function newRepo(world, dir, files) {
  const env = sandboxEnv(world.home);
  const run = (cwd, ...args) => execFileSync("git", args, { cwd, env, stdio: "pipe" });
  for (const [rel, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, rel)), { recursive: true });
    fs.writeFileSync(path.join(dir, rel), text);
  }
  run(dir, "init", "-q", "-b", "main");
  commitAll(world.home, dir, "Seed");
  const bare = path.join(world.remotes, `${path.basename(dir)}.git`);
  run(world.remotes, "init", "-q", "--bare", "-b", "main", bare);
  run(dir, "remote", "add", "origin", bare);
  run(dir, "push", "-q", "-u", "origin", "main");
}

/**
 * A `seedWith` callback for a big store: `skills` more global skills, `notes` more memory
 * notes and enough projects to make `projects` in all (the seed has one).
 */
export function bigStore({ skills = 300, notes = 300, projects = 5 } = {}) {
  return (world) => {
    for (let i = 0; i < skills; i++) {
      const name = `bulk-skill-${String(i).padStart(3, "0")}`;
      const dir = path.join(world.knowledge, "skills", name);
      fs.mkdirSync(dir, { recursive: true });
      fs.writeFileSync(
        path.join(dir, "SKILL.md"),
        `---\nname: ${name}\ndescription: Bulk skill number ${i} for the performance journey\n---\n# ${name}\n\nStep one.\n\nStep two.\n`,
      );
    }
    for (let i = 0; i < notes; i++) {
      const name = `bulk-note-${String(i).padStart(3, "0")}`;
      const next = `bulk-note-${String((i + 1) % notes).padStart(3, "0")}`;
      fs.writeFileSync(
        path.join(world.knowledge, "memory", `${name}.md`),
        `---\nname: ${name}\ndescription: Bulk note ${i}\nmetadata:\n  type: reference\n  host: all\n---\nSee [[${next}]].\n`,
      );
    }
    commitAll(world.home, world.knowledge, "Bulk seed");
    execFileSync("git", ["push", "-q"], { cwd: world.knowledge, env: sandboxEnv(world.home), stdio: "pipe" });
    for (let p = 1; p < projects; p++) {
      newRepo(world, path.join(world.projectsDir, `project-${p}`), {
        "AGENTS.md": `# Project ${p}\n\nKeep it small.\n`,
        [`.agents/skills/tool-${p}/SKILL.md`]: `---\nname: tool-${p}\ndescription: Project ${p} tool\n---\n# tool-${p}\n`,
      });
    }
  };
}
