// atlasd on two fake machines: pair with a code from the Machines page, a dry run that
// writes nothing, a real run that writes every present harness's files (skill links,
// instructions, hooks, subagents) while keeping the owner's own entries, status, drift on
// the Machines page, and the hub going down and coming back.

import fs from "node:fs";
import path from "node:path";
import { test, expect, shot, manualReview } from "../../lib/owner.mjs";
import { machine, startHub, stopHub } from "../../lib/world.mjs";
import {
  atlasd,
  commitFiles,
  copy,
  harnessFiles,
  installHarnesses,
  pairMachine,
  put,
  sendNow,
  updateCount,
} from "../../lib/distribute.mjs";

test.use({ worldOptions: { tag: "atlasd" } });

const SKILLS = ["deploy", "pdf-tools", "review"];
const json = (v) => JSON.stringify(v, null, 2) + "\n";

// The owner's own files, written before atlasd ever runs.
const claudeSettings = {
  theme: "dark",
  permissions: { allow: ["Bash(ls:*)"] },
  hooks: { PreToolUse: [{ matcher: "Edit", hooks: [{ type: "command", command: "echo mine" }] }] },
};
const geminiHooks = { "my-guard": { event: "BeforeTool", command: "echo mine" } };
const opencode = '// my OpenCode config\n{\n  "theme": "tokyonight", // keep this comment\n  "model": "anthropic/claude"\n}\n';
const codexConfig = '# my Codex config\nmodel = "o4"  # keep me\n';
const museSettings = json({ mcpServers: {}, hooks: { Stop: [{ hooks: [{ type: "command", command: "echo muse" }] }] } });
const grokMine = json({ hooks: { Stop: [{ hooks: [{ type: "command", command: "echo grok" }] }] } });
const claudeMd = "# Mine\n\nBe brief.\n";

/** Settings without Atlas's matcher groups, to compare with what the owner wrote. */
function withoutAtlas(settings) {
  const s = structuredClone(settings);
  for (const [event, groups] of Object.entries(s.hooks ?? {})) {
    s.hooks[event] = groups.filter((g) => !g.hooks?.some((h) => h.command?.includes(" # atlas:")));
    if (!s.hooks[event].length) delete s.hooks[event];
  }
  return s;
}

test("laptop: pair, dry run, run, status: every harness gets its files and the owner's stay", async ({ owner, world }) => {
  const m = machine(world, "laptop");
  installHarnesses(m);
  put(m, ".claude/settings.json", json(claudeSettings));
  put(m, ".claude/CLAUDE.md", claudeMd);
  put(m, ".gemini/config/hooks.json", json(geminiHooks));
  put(m, ".config/opencode/opencode.jsonc", opencode);
  put(m, ".codex/config.toml", codexConfig);
  put(m, ".config/muse/settings.json", museSettings);
  put(m, ".grok/hooks/mine.json", grokMine);
  put(m, ".agents/skills/pdf-tools/SKILL.md", "---\nname: pdf-tools\ndescription: my own copy\n---\nMine.\n");

  await sendNow(owner);
  const n = await updateCount(owner);
  expect(n).toBeGreaterThan(0);

  const paired = await pairMachine(owner, world, m);
  expect(paired.out).toContain(`Paired laptop`);
  expect(m.exists(".config/atlas/client.toml")).toBe(true);
  expect(fs.statSync(path.join(m.home, ".config/atlas/client.toml")).mode & 0o777).toBe(0o600);

  // A dry run before the first run has no local copies to plan from, and writes nothing.
  const before = harnessFiles(m);
  const dry = await atlasd(m, ["run", "--dry-run"]);
  expect(dry.code, dry.out).toBe(0);
  expect(dry.out).toContain("has no local copy yet");
  expect(harnessFiles(m), "a dry run writes nothing").toEqual(before);
  expect(m.exists(".local/share/atlas/sources")).toBe(false);
  expect(JSON.parse(m.read(".local/state/atlas-client/state.json")).last_applied).toBeNull();

  const run = await atlasd(m, ["run"]);
  expect(run.code, run.out).toBe(0);
  expect(run.out).toContain(`Update ${n} is applied.`);

  // Skills: one folder per harness, none twice; harnesses that read a linked folder get none.
  const linked = { claude: ".claude/skills", codex: ".agents/skills", pi: ".pi/agent/skills", antigravity: ".gemini/config/skills" };
  for (const [who, dir] of Object.entries(linked)) {
    for (const s of SKILLS) {
      if (who === "codex" && s === "pdf-tools") continue; // the owner's own folder there
      const from = s === "review" ? copy(m, `public-skills/${s}`) : copy(m, `knowledge/skills/${s}`);
      expect(m.link(`${dir}/${s}`), `${who}: ${dir}/${s}`).toBe(from);
      expect(m.exists(`${dir}/${s}/SKILL.md`)).toBe(true);
    }
  }
  for (const dir of [".codex/skills", ".config/opencode/skills", ".config/agents/skills", ".config/amp/skills", ".config/muse/skills", ".grok/skills", ".gemini/antigravity-cli/skills"])
    expect(m.exists(dir), `${dir} gets nothing: its harness reads a linked folder`).toBe(false);
  expect(m.read(".agents/skills/pdf-tools/SKILL.md"), "the owner's own skill folder stays").toContain("Mine.");
  expect(run.out).toContain("~/.agents/skills/pdf-tools is your own folder or file");

  // Instructions: Claude Code imports once; every other harness links the global file.
  const agentsMd = copy(m, "knowledge/AGENTS.md");
  expect(m.read(".claude/CLAUDE.md")).toBe(`${claudeMd}@${agentsMd}\n`);
  for (const f of [".codex/AGENTS.md", ".config/opencode/AGENTS.md", ".pi/agent/AGENTS.md", ".gemini/config/AGENTS.md", ".config/amp/AGENTS.md", ".config/AGENTS.md", ".config/muse/AGENTS.md", ".grok/AGENTS.md"])
    expect(m.link(f), f).toBe(agentsMd);

  // Hooks in each harness's own format; the owner's entries kept byte for byte.
  const script = copy(m, "knowledge/hooks/no-rm/check.sh");
  const atlasGroup = { matcher: "Bash", hooks: [{ type: "command", command: `${script} # atlas:no-rm`, timeout: 10 }] };
  const settings = JSON.parse(m.read(".claude/settings.json"));
  expect(settings.hooks.PreToolUse).toContainEqual(atlasGroup);
  expect(json(withoutAtlas(settings)), "~/.claude/settings.json minus Atlas's entry is the owner's file").toBe(json(claudeSettings));
  expect(JSON.parse(m.read(".codex/hooks.json")).hooks.PreToolUse).toEqual([atlasGroup]);
  const gemini = JSON.parse(m.read(".gemini/config/hooks.json"));
  expect(gemini["my-guard"]).toEqual(geminiHooks["my-guard"]);
  expect(Object.keys(gemini)).toContain("atlas:no-rm");
  expect(JSON.stringify(gemini["atlas:no-rm"])).toContain(script);
  expect(m.exists(".grok/hooks/atlas.json"), "Grok runs Claude's copy, so no second one").toBe(false);
  expect(m.read(".grok/hooks/mine.json")).toBe(grokMine);
  expect(m.read(".config/muse/settings.json"), "Atlas doesn't write Muse hooks").toBe(museSettings);
  expect(m.read(".config/opencode/opencode.jsonc"), "comments and all").toBe(opencode);
  expect(m.read(".codex/config.toml")).toBe(codexConfig);

  // Subagents: Claude Code only.
  expect(m.link(".claude/agents/reviewer.md")).toBe(copy(m, "knowledge/agents/reviewer.md"));

  // Memory goes straight to git (decision 0008): host notes arrive in the pinned copy only.
  expect(fs.existsSync(copy(m, "knowledge/memory/hosts/laptop/disk.md"))).toBe(true);

  // A second run changes nothing.
  const after = harnessFiles(m);
  const again = await atlasd(m, ["run"]);
  expect(again.code, again.out).toBe(0);
  expect(harnessFiles(m), "idempotent").toEqual(after);

  // Drift: a removed link shows in a dry run without being made, and a run repairs it.
  fs.rmSync(path.join(m.home, ".pi/agent/skills/deploy"));
  const drift = harnessFiles(m);
  const plan = await atlasd(m, ["run", "--dry-run"]);
  expect(plan.out).toContain(`would link ~/.pi/agent/skills/deploy → ~/.local/share/atlas/sources/knowledge/skills/deploy`);
  expect(harnessFiles(m)).toEqual(drift);
  await atlasd(m, ["run"]);
  expect(m.link(".pi/agent/skills/deploy")).toBe(copy(m, "knowledge/skills/deploy"));

  const status = await atlasd(m, ["status"]);
  expect(status.code, status.out).toBe(0);
  expect(status.out).toContain(`Hub: ${world.url}`);
  expect(status.out).toContain("Machine: laptop");
  expect(status.out).toContain(`Last applied update: ${n}`);
  expect(status.out).toContain("Trusted passkeys: Verify laptop");
  expect(status.out).toContain("Nothing to change.");
  test.info().attach("atlasd output", { body: [paired.out, dry.out, run.out, plan.out, status.out].join("\n---\n") });

  // The Machines page shows what laptop reported.
  await owner.goto("/machines");
  const ledger = owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "laptop", exact: true }) });
  await expect(ledger.locator(".ledger-sum")).toHaveText("1 paired");
  await expect(ledger.locator(".client-row")).toContainText(`Update ${n}, up to date · seen just now`);
  await expect(ledger.locator(".client-row")).toContainText("your own folder or file");
  const claude = ledger.locator("details.row", { hasText: "Claude" });
  await expect(claude).toContainText("3 skills from Atlas");
  await expect(claude).toContainText("Global AGENTS.md imported");
  await expect(claude).toContainText("1 hook · 1 subagent");
  await expect(ledger.locator("details.row")).toHaveCount(8);
  await shot(owner, "machines: laptop reported");

  // A newer update shows laptop behind until it runs.
  commitFiles(world, world.knowledge, { "skills/release-notes/SKILL.md": "---\nname: release-notes\ndescription: Write release notes\n---\n# release-notes\n" }, "Add a skill by hand");
  await sendNow(owner);
  await owner.goto("/machines");
  await expect(ledger.locator(".client-row")).toContainText(`Update ${n}, 1 behind`);
  await shot(owner, "machines: laptop one update behind");
  const caught = await atlasd(m, ["run"]);
  expect(caught.out).toContain(`Update ${n + 1} is applied.`);
  expect(m.link(".claude/skills/release-notes")).toBe(copy(m, "knowledge/skills/release-notes"));
  await owner.reload();
  await expect(ledger.locator(".client-row")).toContainText(`Update ${n + 1}, up to date`);
});

test("harnesses that read both Claude's and Codex's folders see each skill once", async ({ owner, world }) => {
  const m = machine(world, "twice");
  installHarnesses(m, ["claude", "codex", "opencode"]);
  await sendNow(owner);
  await pairMachine(owner, world, m);
  expect((await atlasd(m, ["run"])).code).toBe(0);
  await owner.goto("/machines");
  const ledger = owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "twice", exact: true }) });
  await shot(owner, "machines: skill counts per harness");
  const skills = countSkills(world);
  await expect(ledger.locator("details.row", { hasText: "OpenCode" })).toContainText(`${skills} skills from Atlas`, { timeout: 2000 });
});

/** Skills in the latest update: Knowledge's plus public skills not shadowed by one. */
function countSkills(world) {
  const names = new Set([
    ...fs.readdirSync(path.join(world.knowledge, "skills")),
    ...fs.readdirSync(world.publicSkills).filter((n) => fs.existsSync(path.join(world.publicSkills, n, "SKILL.md"))),
  ]);
  return names.size;
}

test("box: only Codex, OpenCode and Grok Build; Grok gets its own hook file", async ({ owner, world }) => {
  const m = machine(world, "box");
  installHarnesses(m, ["codex", "opencode", "grok"]);
  await sendNow(owner);
  const n = await updateCount(owner);
  await pairMachine(owner, world, m);
  const run = await atlasd(m, ["run"]);
  expect(run.code, run.out).toBe(0);
  expect(run.out).toContain(`Update ${n} is applied.`);

  for (const s of ["deploy", "pdf-tools"]) expect(m.link(`.agents/skills/${s}`)).toBe(copy(m, `knowledge/skills/${s}`));
  expect(m.link(".agents/skills/review")).toBe(copy(m, "public-skills/review"));
  for (const dir of [".claude", ".pi", ".gemini", ".config/amp", ".config/muse", ".grok/skills", ".config/opencode/skills"])
    expect(m.exists(dir), `${dir}: absent or already covered`).toBe(false);
  const agentsMd = copy(m, "knowledge/AGENTS.md");
  for (const f of [".codex/AGENTS.md", ".config/opencode/AGENTS.md", ".grok/AGENTS.md"]) expect(m.link(f), f).toBe(agentsMd);

  const script = copy(m, "knowledge/hooks/no-rm/check.sh");
  const group = { matcher: "Bash", hooks: [{ type: "command", command: `${script} # atlas:no-rm`, timeout: 10 }] };
  expect(JSON.parse(m.read(".grok/hooks/atlas.json")).hooks.PreToolUse, "no Claude Code here, so Grok's own file").toEqual([group]);
  expect(JSON.parse(m.read(".codex/hooks.json")).hooks.PreToolUse).toEqual([group]);

  await owner.goto("/machines");
  const ledger = owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "box", exact: true }) });
  await expect(ledger.locator(".client-row")).toContainText(`Update ${n}, up to date`);
  await expect(ledger.locator("details.row")).toHaveCount(3);
  await expect(ledger.locator("details.row", { hasText: "Grok" })).toContainText("1 hook");
  await shot(owner, "machines: box reported");
});

test("hub down: harness files stay, atlasd says why, and recovers when the hub is back", async ({ owner, world }) => {
  const m = machine(world, "laptop-offline");
  installHarnesses(m, ["claude", "codex"]);
  await sendNow(owner);
  const n = await updateCount(owner);
  await pairMachine(owner, world, m);
  expect((await atlasd(m, ["run"])).code).toBe(0);
  const files = harnessFiles(m);
  const state = m.read(".local/state/atlas-client/state.json");

  await stopHub(world);
  try {
    const down = await atlasd(m, ["run"]);
    test.info().attach("atlasd with the hub down", { body: down.out });
    expect(down.code, "a down hub isn't a failed run").toBe(0);
    expect(down.out).toContain("couldn't reach the hub");
    expect(down.out).toContain(`agents keep using the local copies (update ${n})`);
    expect(harnessFiles(m), "nothing changes").toEqual(files);
    expect(m.read(".local/state/atlas-client/state.json")).toBe(state);
    const status = await atlasd(m, ["status"]);
    expect(status.out).toContain(`Last applied update: ${n}`);
    expect(status.out).toContain("couldn't reach the hub");
  } finally {
    await startHub(world);
  }

  // Decision 0034: the owner's passkey session survives the hub restarting.
  await owner.goto("/");
  await expect(owner.locator('meta[name="atlas-csrf"]'), "still signed in after the restart").toHaveCount(1);
  commitFiles(world, world.knowledge, { "skills/after-outage/SKILL.md": "---\nname: after-outage\ndescription: Added while a machine was offline\n---\n# after-outage\n" }, "A skill while the hub was down");
  await sendNow(owner);
  const back = await atlasd(m, ["run"]);
  expect(back.code, back.out).toBe(0);
  expect(back.out).toContain(`Update ${n + 1} is applied.`);
  expect(m.link(".claude/skills/after-outage")).toBe(copy(m, "knowledge/skills/after-outage"));
  await owner.goto("/machines");
  await expect(owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "laptop-offline", exact: true }) }).locator(".client-row")).toContainText(`Update ${n + 1}, up to date`);
  await shot(owner, "machines after the hub came back");
});

test("a hook whose script isn't executable is flagged before sending and skipped on machines", async ({ owner, world }) => {
  const m = machine(world, "laptop-loose");
  installHarnesses(m, ["claude"]);
  await pairMachine(owner, world, m);
  // The owner forgets chmod +x: the script is committed 100644.
  commitFiles(world, world.knowledge, {
    "hooks/loose/hook.toml": 'description = "Loose"\nevent = "pre-tool-use"\nmatcher = "Bash"\nrun = "run.sh"\n',
    "hooks/loose/run.sh": "#!/bin/sh\nexit 0\n",
  }, "A hook without the execute bit");
  manualReview(owner);
  try {
    await owner.goto("/settings");
    await owner.locator("#updates").getByRole("button", { name: "Send now" }).click();
    await owner.waitForURL(/\/send\/review\//);
    const loose = owner.locator("li", { hasText: "loose" });
    await expect(loose).toContainText("Its script isn't marked as runnable, so your machines will skip this hook.");
    await shot(owner, "check before sending flags the hook");
    await owner.locator("section[data-pending] [data-action='send']").click();
    await owner.waitForURL(/flash=sent/);
  } finally {
    manualReview(owner, false);
  }
  const run = await atlasd(m, ["run"]);
  test.info().attach("atlasd run", { body: run.out, contentType: "text/plain" });
  expect(run.out).toContain("its script run.sh isn't executable (mark it with chmod +x and commit)");
  // Atlas marks each command it manages with "# atlas:<name>".
  expect(m.read(".claude/settings.json")).not.toContain("# atlas:loose");
  expect(m.read(".claude/settings.json")).toContain("# atlas:no-rm");
});
