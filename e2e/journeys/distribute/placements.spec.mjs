// Where skills go (documents/skills.md, decision 0027): per-agent switches and
// "only these projects" in <knowledge>/atlas/placements.toml, carried by Send and applied
// by atlasd, and project checkouts on a machine getting their project skills linked.
// The hub has no UI for switches yet, so the owner edits the plain file (a supported way
// to change Atlas) and commits it.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { machine, sandboxEnv } from "../../lib/world.mjs";
import { atlasd, commitFiles, copy, installHarnesses, pairMachine, put, sendNow } from "../../lib/distribute.mjs";

test.use({ worldOptions: { tag: "placements" } });

// The folders each agent reads skills from in the home folder (atlas_harness::layouts).
const READS = {
  "claude-code": [".claude/skills"],
  codex: [".agents/skills", ".codex/skills"],
  "open-code": [".config/opencode/skills", ".claude/skills", ".agents/skills"],
  pi: [".pi/agent/skills"],
};

const opencodeText = '// my OpenCode config\n{\n  "theme": "tokyonight", // keep this comment\n  "permission": {\n    "bash": "ask" // mine\n  }\n}\n';
const codexText = '# my Codex config\nmodel = "o4"  # keep me\n';
const piText = JSON.stringify({ defaultModel: "sonnet" }, null, 2) + "\n";

/** Whether `agent` has switched skill `name` off in its own settings on machine `m`. */
function switchedOff(m, agent, name) {
  const read = (rel) => (m.exists(rel) ? m.read(rel) : "");
  switch (agent) {
    case "claude-code":
      return JSON.parse(read(".claude/settings.json") || "{}").skillOverrides?.[name] === "off";
    case "codex":
      return new RegExp(`\\[\\[skills\\.config\\]\\][^\\[]*${name}/SKILL\\.md[^\\[]*enabled = false`).test(read(".codex/config.toml"));
    case "open-code":
      return new RegExp(`"${name}": "deny"`).test(read(".config/opencode/opencode.jsonc"));
    case "pi":
      return (JSON.parse(read(".pi/agent/settings.json") || "{}").skills ?? []).some((s) => s.startsWith("-") && s.includes(`${name}/SKILL.md`));
    default:
      return false;
  }
}

/** Whether `agent` would load skill `name` from its home folders on machine `m`. */
const sees = (m, agent, name) => READS[agent].some((d) => m.exists(`${d}/${name}/SKILL.md`)) && !switchedOff(m, agent, name);

test("the skill page has no per-agent switches yet", async ({ owner }) => {
  await owner.goto("/skills");
  const row = owner.locator("details.row", { has: owner.locator(".row-name", { hasText: /^pdf-tools$/ }) }).first();
  await row.locator("summary").click();
  await row.getByRole("link", { name: "Read the skill" }).click();
  await expect(owner.locator("header").getByRole("heading", { name: "pdf-tools" })).toBeVisible();
  // Agents show as read-only states; nothing turns one off.
  await expect(owner.locator(".agent-states").first()).toBeVisible();
  await shot(owner, "skill page without switches");
  await expect(owner.getByRole("switch")).toHaveCount(0);
  await expect(owner.locator("main input[type=checkbox]")).toHaveCount(0);
});

test("switches in placements.toml reach each agent's own setting; the owner's entries stay", async ({ owner, world }) => {
  test.info().annotations.push({
    type: "bug",
    description:
      "atlasd ignores atlas/placements.toml: it links every skill for every agent and writes no switches (skillOverrides, [[skills.config]], permission.skill, Pi skills), although documents/skills.md and decision 0027 say Send carries placements and atlasd applies them (crates/atlas-client has no placement code; skills_place::set_agents is a stub).",
  });
  test.fail();
  const m = machine(world, "laptop");
  installHarnesses(m, ["claude", "codex", "opencode", "pi"]);
  put(m, ".config/opencode/opencode.jsonc", opencodeText);
  put(m, ".codex/config.toml", codexText);
  put(m, ".pi/agent/settings.json", piText);

  const head = commitFiles(world, world.knowledge, {
    "atlas/placements.toml": '[global.pdf-tools]\noff = ["open-code"]\n\n[global.deploy]\noff = ["claude-code", "pi"]\n',
  }, "Switch pdf-tools off for OpenCode and deploy off for Claude Code and Pi");
  await sendNow(owner);
  await pairMachine(owner, world, m);
  const run = await atlasd(m, ["run"]);
  test.info().attach("atlasd run", { body: run.out });
  expect(run.code, run.out).toBe(0);
  expect(fs.readFileSync(copy(m, "knowledge/atlas/placements.toml"), "utf8"), "the update carried the file").toContain("open-code");
  expect(head).toBeTruthy();

  // Every format keeps what the owner wrote.
  for (const line of opencodeText.split("\n").filter(Boolean).filter((l) => !/^\s*}\s*$/.test(l)))
    expect(m.read(".config/opencode/opencode.jsonc"), "OpenCode keeps the owner's lines and comments").toContain(line);
  expect(m.read(".codex/config.toml").startsWith(codexText)).toBe(true);
  expect(JSON.parse(m.read(".pi/agent/settings.json")).defaultModel).toBe("sonnet");

  for (const agent of ["claude-code", "codex", "pi"]) expect(sees(m, agent, "pdf-tools"), `${agent} keeps pdf-tools`).toBe(true);
  expect(sees(m, "open-code", "pdf-tools"), "OpenCode doesn't load pdf-tools").toBe(false);
  for (const agent of ["codex", "open-code"]) expect(sees(m, agent, "deploy"), `${agent} keeps deploy`).toBe(true);
  for (const agent of ["claude-code", "pi"]) expect(sees(m, agent, "deploy"), `${agent} doesn't load deploy`).toBe(false);
});

test("a checkout of shop gets its project skill linked, kept out of git", async ({ owner, world }) => {
  test.info().annotations.push({
    type: "bug",
    description:
      "Project checkouts get nothing: the update leaves projects out of the manifest (atlas-web release::build, 'projects never go in') and atlasd has no project_roots scan, although documents/skills.md ('Projects on each machine') and decision 0027 say an update names each project and its remote and atlasd links project skills into .claude/skills with .git/info/exclude.",
  });
  test.fail();
  const m = machine(world, "box");
  installHarnesses(m, ["claude", "codex"]);
  const checkout = path.join(m.home, "Projects/shop");
  execFileSync("git", ["clone", "-q", path.join(world.remotes, "shop.git"), checkout], { env: sandboxEnv(m.home) });

  commitFiles(world, world.knowledge, { "atlas/placements.toml": '[global.review]\nonly-projects = ["shop"]\n' }, "review only in shop");
  await sendNow(owner);
  await pairMachine(owner, world, m);
  const run = await atlasd(m, ["run"]);
  test.info().attach("atlasd run", { body: run.out });
  expect(run.code, run.out).toBe(0);

  // Claude Code doesn't read .agents/skills, so the project skill is linked for it.
  expect(fs.readlinkSync(path.join(checkout, ".claude/skills/stock"))).toBe("../../.agents/skills/stock");
  expect(fs.existsSync(path.join(checkout, ".claude/skills/stock/SKILL.md"))).toBe(true);
  const exclude = fs.readFileSync(path.join(checkout, ".git/info/exclude"), "utf8");
  expect(exclude).toContain(".claude/skills/stock");
  expect(execFileSync("git", ["status", "--porcelain"], { cwd: checkout, env: sandboxEnv(m.home), encoding: "utf8" }), "the checkout stays clean").toBe("");

  // review is used in shop only: linked into the checkout, not the home folders.
  expect(m.exists(".claude/skills/review")).toBe(false);
  expect(m.exists(".agents/skills/review")).toBe(false);
  expect(fs.existsSync(path.join(checkout, ".claude/skills/review/SKILL.md"))).toBe(true);
});
