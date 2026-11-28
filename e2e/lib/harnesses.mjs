// The real harnesses, asked offline what they loaded from a machine's HOME: the outcome
// check behind every file atlasd writes. Each probe runs the installed binary (resolved once
// with `mise which` under the real HOME) with the machine's sandbox env, no API keys, a hard
// timeout and, where it works, no network (`unshare -rn`). Claude Code needs its own network
// stack to start, so it gets a dead API address instead. Everything a probe starts is killed
// through its child handle (its process group), never by name.
//
// Also the small Send and pair steps the harness journey needs.

import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import path from "node:path";
import { RUN_DIR, BIN, sandboxEnv, hubCli } from "./world.mjs";
import { expect } from "./owner.mjs";

/** Atlas's agent ids (as in `atlas/placements.toml`) and the binary each runs as. */
export const HARNESSES = {
  "claude-code": { bin: "claude", marker: ".claude" },
  codex: { bin: "codex", marker: ".codex" },
  "open-code": { bin: "opencode", marker: ".config/opencode" },
  pi: { bin: "pi", marker: ".pi/agent" },
  amp: { bin: "amp", marker: ".config/amp" },
  muse: { bin: "muse", marker: ".config/muse" },
  grok: { bin: "grok", marker: ".grok" },
  antigravity: { bin: "agy", marker: ".gemini" },
};

// The markers are `Layout::marker` in crates/atlas-harness/src/lib.rs: atlasd treats a
// harness as set up for a login when that path exists.
export function createMarkers(home) {
  for (const { marker } of Object.values(HARNESSES)) fs.mkdirSync(path.join(home, marker), { recursive: true });
}

// ---------------------------------------------------------------- the owner's steps

/** Clicks Send on the front page and taps the passkey; lands on `/?sent=1`. */
export async function sendFromFrontPage(page) {
  await page.goto("/");
  const send = page.locator('[data-action="send"]').first();
  await expect(send).toBeVisible();
  await send.click();
  await page.waitForURL((u) => u.searchParams.has("sent"));
  await expect(page.getByText("On your machines within a minute")).toBeVisible();
}

/** A one-time pairing code from `atlas-hub pair-code` (the end of its install command). */
export function pairCode(world) {
  const out = hubCli(world, "pair-code");
  const m = out.match(/\| sh -s -- (\S+)/);
  if (!m) throw new Error(`no pairing code in:\n${out}`);
  return m[1];
}

/**
 * atlasd as `m`'s login, without blocking: the world's proxy runs in this process, so the
 * synchronous `m.atlasd()` can't reach the hub. Resolves to { code, out }.
 */
export function atlasd(m, ...args) {
  return new Promise((resolve) => {
    // USER names the login on the Machines page, as it does on a real machine.
    const env = sandboxEnv(m.home, { USER: "verify" });
    const child = spawn(path.join(BIN, "atlasd"), args, { env, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (out += d));
    child.on("close", (code) => resolve({ code, out }));
  });
}

/** `atlasd pair` for `m`; throws with atlasd's output when it fails. */
export async function pair(world, m, name = m.name) {
  const r = await atlasd(m, "pair", world.url, pairCode(world), "--machine", name);
  if (r.code !== 0) throw new Error(`atlasd pair failed:\n${r.out}`);
  return r.out;
}

// ---------------------------------------------------------------- running a harness

const BIN_DIR = path.join(RUN_DIR, "harness-bin");
let resolved = null;

/** Real binaries by name (null when not installed), symlinked into one bin dir per run. */
export function harnessBins() {
  if (resolved) return resolved;
  fs.mkdirSync(BIN_DIR, { recursive: true });
  resolved = {};
  const which = (cmd, args) => {
    try {
      return execFileSync(cmd, args, { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 20000 }).trim();
    } catch {
      return null;
    }
  };
  // OpenCode and Amp are node scripts, so node goes in the bin dir too.
  for (const bin of [...Object.values(HARNESSES).map((h) => h.bin), "node"]) {
    // mise shims re-download under a sandbox HOME, so resolve the real file with the real HOME.
    let real = which("mise", ["which", bin]) ?? which("which", [bin]);
    if (real && (real.includes("/mise/shims/") || !fs.existsSync(real))) real = null;
    resolved[bin] = real;
    if (!real) continue;
    const link = path.join(BIN_DIR, bin);
    fs.rmSync(link, { force: true });
    try {
      fs.symlinkSync(real, link);
    } catch (e) {
      if (e.code !== "EEXIST") throw e;
    }
  }
  return resolved;
}

let unshareWorks = null;
function canUnshare() {
  if (unshareWorks === null) {
    try {
      execFileSync("unshare", ["-rn", "true"], { stdio: "ignore", timeout: 5000 });
      unshareWorks = true;
    } catch {
      unshareWorks = false;
    }
  }
  return unshareWorks;
}

/**
 * Runs `args` as `home`'s login and resolves to { code, out, err, timedOut, cmd }.
 * Offline by default: an empty network namespace, with loopback up when `loopback` (for a
 * harness that talks to its own local server). `talk(child, line, done)` sees stdout line
 * by line; `start(child, done)` runs once it is spawned. `done()` ends the run. The whole
 * process group is killed at the end or on `timeout`.
 */
function run(args, { home, cwd, env = {}, timeout = 60000, net = false, loopback = false, talk, start } = {}) {
  let argv = args;
  if (!net && canUnshare())
    argv = loopback
      ? ["unshare", "-rn", "sh", "-c", 'ip link set lo up 2>/dev/null; exec "$@"', "sh", ...args]
      : ["unshare", "-rn", ...args];
  return new Promise((resolve) => {
    // sandboxEnv starts empty, so no API key, CLAUDECODE, CLAUDE_CODE_* or MISE_SHELL leaks in.
    const child = spawn(argv[0], argv.slice(1), {
      cwd,
      env: sandboxEnv(home, { PATH: `${BIN_DIR}:/usr/bin:/bin`, TERM: "dumb", NO_COLOR: "1", ...env }),
      stdio: ["pipe", "pipe", "pipe"],
      detached: true,
    });
    let out = "";
    let err = "";
    let buf = "";
    let timedOut = false;
    const kill = () => {
      try {
        process.kill(-child.pid, "SIGKILL");
      } catch {}
    };
    const timer = setTimeout(() => {
      timedOut = true;
      kill();
    }, timeout);
    child.stdout.on("data", (d) => {
      out += d;
      if (!talk) return;
      buf += d;
      for (let i; (i = buf.indexOf("\n")) >= 0; ) {
        const line = buf.slice(0, i);
        buf = buf.slice(i + 1);
        if (line.trim()) talk(child, line, kill);
      }
    });
    child.stderr.on("data", (d) => (err += d));
    child.stdin.on("error", () => {});
    if (start) start(child, kill);
    else if (!talk) child.stdin.end();
    child.on("close", (code) => {
      clearTimeout(timer);
      kill(); // whatever the harness left running in its group
      resolve({ code, out, err, timedOut, cmd: args.join(" ") });
    });
  });
}

const parse = (text) => {
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
};
const lines = (text) => text.split("\n").map(parse).filter(Boolean);

/**
 * JSON-RPC over stdio, one request at a time (`[method, params]`; `[method, params, true]`
 * is a notification). Ends `settle` ms after the last answer, so late notifications arrive.
 */
async function rpc(args, steps, { settle = 0, ...opts }) {
  const results = {};
  const notes = [];
  let i = 0;
  const next = (child, done) => {
    while (i < steps.length) {
      const [method, params, notify] = steps[i++];
      const msg = { jsonrpc: "2.0", method, params };
      if (!notify) msg.id = i;
      child.stdin.write(JSON.stringify(msg) + "\n");
      if (!notify) return;
    }
    setTimeout(done, settle);
  };
  const r = await run(args, {
    ...opts,
    start: next,
    talk: (child, line, done) => {
      const m = parse(line);
      if (!m) return;
      if (m.id !== undefined && ("result" in m || "error" in m)) {
        results[steps[m.id - 1]?.[0] ?? m.id] = m.error ? { error: m.error } : m.result;
        if (m.id === i) next(child, done);
      } else notes.push(m);
    },
  });
  return { ...r, results, notes };
}

/** A fresh folder for a probe's cwd that isn't inside any repo or the real HOME. */
export function neutralDir() {
  const dir = fs.mkdtempSync(path.join(process.env.XDG_RUNTIME_DIR ?? "/tmp", "av-probe-"));
  execFileSync("git", ["init", "-q", dir], { stdio: "ignore" });
  return dir;
}

const real = (p) => {
  try {
    return fs.realpathSync(p);
  } catch {
    return p;
  }
};

// ---------------------------------------------------------------- one probe per harness
//
// Each returns { skills, off, instructions, hooks, agents, gaps, raw }: skill names the
// harness lists; those it lists but reports switched off; instruction file paths it says it
// loaded (null when it can't say); hooks it sees as { event, matcher, atlas } (null when it
// can't say, or Atlas writes it none); subagent names (null where Atlas writes none).

// A fake Anthropic API on loopback that asks for one Bash call, then ends the turn, so
// Claude Code's PreToolUse hooks run offline and show in its own event stream.
function fakeModel() {
  const msg = (content, stop) => ({
    id: "msg_probe",
    type: "message",
    role: "assistant",
    model: "claude-opus-5-5",
    content,
    stop_reason: stop,
    stop_sequence: null,
    usage: { input_tokens: 1, output_tokens: 1 },
  });
  const server = http.createServer((req, res) => {
    let body = "";
    req.on("data", (d) => (body += d));
    req.on("end", () => {
      const j = parse(body) ?? {};
      if (!req.url.startsWith("/v1/messages") || req.url.includes("count_tokens")) {
        res.writeHead(200, { "content-type": "application/json" });
        return res.end(JSON.stringify({ input_tokens: 1 }));
      }
      const answered = JSON.stringify(j.messages?.at(-1) ?? "").includes("tool_result");
      const tool = !answered && (j.tools ?? []).some((t) => t.name === "Bash");
      const input = { command: "echo atlas-probe", description: "Atlas probe" };
      const block = tool ? { type: "tool_use", id: "toolu_probe", name: "Bash", input } : { type: "text", text: "done" };
      const stop = tool ? "tool_use" : "end_turn";
      if (!j.stream) {
        res.writeHead(200, { "content-type": "application/json" });
        return res.end(JSON.stringify(msg([block], stop)));
      }
      const events = [
        { type: "message_start", message: msg([], null) },
        { type: "content_block_start", index: 0, content_block: tool ? { ...block, input: {} } : { type: "text", text: "" } },
        {
          type: "content_block_delta",
          index: 0,
          delta: tool ? { type: "input_json_delta", partial_json: JSON.stringify(input) } : { type: "text_delta", text: "done" },
        },
        { type: "content_block_stop", index: 0 },
        { type: "message_delta", delta: { stop_reason: stop, stop_sequence: null }, usage: { output_tokens: 1 } },
        { type: "message_stop" },
      ];
      res.writeHead(200, { "content-type": "text/event-stream" });
      for (const e of events) res.write(`event: ${e.type}\ndata: ${JSON.stringify(e)}\n\n`);
      res.end();
    });
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve(server)));
}

async function claudeCode({ home, cwd }) {
  // Claude Code won't start without a network stack, so it runs online with a dead API
  // address: `/context` is answered locally.
  const offline = { ANTHROPIC_BASE_URL: "http://127.0.0.1:9", ANTHROPIC_API_KEY: "sk-ant-dummy" };
  const ctx = await run(["claude", "-p", "/context", "--output-format", "stream-json", "--verbose"], {
    home,
    cwd,
    net: true,
    env: offline,
  });
  const init = lines(ctx.out).find((l) => l.subtype === "init") ?? {};
  const memory = [...ctx.out.matchAll(/"memory_files":(\[[^\]]*\])/g)].flatMap((m) => parse(m[1]) ?? []);
  // Hooks run only around a tool call, so a fake model on loopback asks for one.
  const server = await fakeModel();
  const turn = await run(
    ["claude", "-p", "Run the probe command.", "--output-format", "stream-json", "--verbose", "--include-hook-events", "--max-turns", "3"],
    { home, cwd, net: true, env: { ...offline, ANTHROPIC_BASE_URL: `http://127.0.0.1:${server.address().port}` } },
  );
  await new Promise((r) => server.close(r));
  const events = lines(turn.out).filter((l) => l.subtype === "hook_started" || l.subtype === "hook_response");
  const hooks = events
    .filter((e) => e.subtype === "hook_started" && !e.hook_name.startsWith("SessionStart"))
    .map((e) => {
      const [event, matcher] = e.hook_name.split(":");
      const done = events.find((r) => r.subtype === "hook_response" && r.hook_id === e.hook_id);
      return { event, matcher, atlas: null, ran: done ? { exit: done.exit_code, outcome: done.outcome, output: done.output } : null };
    });
  return {
    skills: init.skills ?? [],
    off: [],
    instructions: memory.map((m) => m.path),
    hooks,
    agents: init.agents ?? [],
    gaps: [],
    raw: `$ ${ctx.cmd}\n${ctx.out}${ctx.err}\n\n$ ${turn.cmd}  (fake model on loopback)\n${turn.out}${turn.err}`,
  };
}

async function codex({ home, cwd, marker }) {
  const env = { CODEX_HOME: path.join(home, ".codex") };
  const r = await rpc(
    ["codex", "app-server"],
    [
      ["initialize", { clientInfo: { name: "atlas-verify", version: "1" } }],
      ["initialized", {}, true],
      ["skills/list", { cwds: [cwd] }],
      ["hooks/list", { cwds: [cwd] }],
    ],
    { home, cwd, env },
  );
  const skills = (r.results["skills/list"]?.data ?? []).flatMap((d) => d.skills);
  const hooks = (r.results["hooks/list"]?.data ?? []).flatMap((d) => d.hooks);
  const prompt = await run(["codex", "debug", "prompt-input"], { home, cwd, env });
  const found = marker && prompt.out.includes(marker);
  return {
    skills: skills.map((s) => s.name),
    off: skills.filter((s) => s.enabled === false).map((s) => s.name),
    // Codex shows the instructions' text, not the file it read; the home file is the only
    // one atlasd writes for it.
    instructions: found ? [path.join(home, ".codex/AGENTS.md")] : [],
    hooks: hooks.map((h) => ({
      event: h.eventName,
      matcher: h.matcher,
      atlas: h.command?.match(/# atlas:(\S+)/)?.[1] ?? null,
      trust: h.trustStatus,
    })),
    agents: null,
    gaps: ["Codex sees Atlas's hooks as untrusted: they don't run until the owner trusts them in Codex."],
    raw: `$ ${r.cmd}\n${r.out}${r.err}\n\n$ ${prompt.cmd}  (searched for ${JSON.stringify(marker)}: ${found ? "found" : "not found"})\n${prompt.out.length} bytes`,
  };
}

async function openCode({ home, cwd }) {
  // `opencode api` talks to a running `opencode serve`, and both need the same loopback, so
  // one shell in the namespace starts the server, polls until the list settles, asks for the
  // agents' permissions and stops the server it started.
  const script = [
    "OPENCODE_PASSWORD=atlas-probe opencode serve --hostname 127.0.0.1 --port 47131 >/dev/null 2>&1 & SP=$!",
    'prev=""; for i in $(seq 1 20); do sleep 1;',
    '  out=$(OPENCODE_PASSWORD=atlas-probe timeout 10 opencode api --server http://127.0.0.1:47131 GET /api/skill 2>&1)',
    '  [ $i -ge 4 ] && [ "$out" = "$prev" ] && break; prev="$out"; done',
    'echo "$out"; echo "---- debug agents"',
    "timeout 20 opencode debug agents 2>&1",
    "kill $SP",
  ].join("\n");
  const r = await run(["sh", "-c", script], { home, cwd, loopback: true, timeout: 90000 });
  const [list, agentsText = ""] = r.out.split("---- debug agents");
  const skills = (parse(list.trim())?.data ?? []).filter((s) => !String(s.path).startsWith("skill://"));
  const build = (parse(agentsText.trim()) ?? []).find((a) => a.id === "build");
  const off = (build?.permissions ?? [])
    .filter((p) => p.action === "skill" && p.effect === "deny" && p.resource !== "*")
    .map((p) => p.resource);
  return {
    skills: skills.map((s) => s.id ?? s.name),
    off,
    instructions: null,
    hooks: null,
    agents: null,
    gaps: ["OpenCode has no offline way to say which instruction files it loaded."],
    raw: `$ ${r.cmd}\n${r.out}${r.err}`,
  };
}

// A Pi extension that registers /atlas-probe, which writes what Pi put in its prompt.
const PI_EXTENSION = `import { writeFileSync } from "node:fs";
export default function (pi: any) {
  pi.registerCommand("atlas-probe", {
    description: "dump loaded context files and skills",
    handler: async (_args: string, ctx: any) => {
      const o = ctx.getSystemPromptOptions();
      writeFileSync(process.env.ATLAS_PROBE_OUT!, JSON.stringify({
        contextFiles: (o.contextFiles ?? []).map((f: any) => f.path),
        skills: (o.skills ?? []).map((s: any) => s.name ?? s),
        promptHasMarker: !!process.env.ATLAS_PROBE_MARKER && ctx.getSystemPrompt().includes(process.env.ATLAS_PROBE_MARKER),
      }));
    },
  });
}
`;

async function pi({ home, cwd, marker, trust }) {
  const ext = path.join(BIN_DIR, "pi-probe-ext.ts");
  fs.writeFileSync(ext, PI_EXTENSION);
  const outFile = path.join(fs.mkdtempSync(path.join(process.env.XDG_RUNTIME_DIR ?? "/tmp", "av-pi-")), "probe.json");
  const steps = [
    { type: "get_commands", id: "1" },
    { type: "prompt", id: "2", message: "/atlas-probe" },
  ];
  const replies = {};
  let i = 0;
  const send = (child, done) => (i < steps.length ? child.stdin.write(JSON.stringify(steps[i++]) + "\n") : done());
  const r = await run(["pi", "--mode", "rpc", "--offline", "--no-session", ...(trust ? ["--approve"] : []), "-e", ext], {
    home,
    cwd,
    env: { ATLAS_PROBE_OUT: outFile, ATLAS_PROBE_MARKER: marker ?? "" },
    start: send,
    talk: (child, line, done) => {
      const m = parse(line);
      if (m?.type === "response") {
        replies[m.command] = m;
        send(child, done);
      }
    },
  });
  const commands = replies.get_commands?.data?.commands ?? [];
  const dump = parse(fs.existsSync(outFile) ? fs.readFileSync(outFile, "utf8") : "") ?? {};
  fs.rmSync(path.dirname(outFile), { recursive: true, force: true });
  return {
    skills: commands.filter((c) => c.source === "skill").map((c) => c.name.replace(/^skill:/, "")),
    off: [],
    instructions: dump.contextFiles ?? [],
    promptHasMarker: dump.promptHasMarker ?? false,
    hooks: null,
    agents: null,
    gaps: [],
    raw: `$ ${r.cmd}\n${r.out}${r.err}\n\n/atlas-probe wrote:\n${JSON.stringify(dump, null, 2)}`,
  };
}

async function amp({ home, cwd, check = [] }) {
  // Without a key Amp starts a sign-in flow and waits; a dummy one is never used offline.
  const env = { AMP_API_KEY: "sgamp_dummy" };
  const list = await run(["amp", "skill", "list", "--json"], { home, cwd, env });
  const skills = parse(list.out)?.skills ?? [];
  let raw = `$ ${list.cmd}\n${list.out}${list.err}`;
  const off = [];
  for (const name of check) {
    const t = await run(["amp", "permissions", "test", "skill", "--json", "--name", name], { home, cwd, env });
    raw += `\n\n$ ${t.cmd}\n${t.out}${t.err}`;
    if (parse(t.out)?.action === "reject") off.push(name);
  }
  return {
    skills: skills.map((s) => s.name),
    off,
    instructions: null,
    hooks: null,
    agents: null,
    gaps: ["Amp has no offline way to say which instruction files it loaded."],
    raw,
  };
}

async function muse({ home, cwd, trust }) {
  const t = trust ? ["--trust-workspace"] : [];
  const list = await run(["muse", "skills", "list", ...t, "--json"], { home, cwd });
  const skills = (parse(list.out)?.skills ?? []).filter((s) => s.scope !== "bundled" && s.scope !== "plugin");
  // The offline echo provider runs one turn and records the prompt it would have sent.
  const turn = await run(["muse", "exec", "--provider", "echo", ...t, "--json", "hi"], { home, cwd });
  const session = lines(turn.out).find((l) => l.stream?.kind === "session")?.stream.id;
  let text = "";
  if (session) {
    const find = (dir) => {
      for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
        const p = path.join(dir, e.name);
        if (e.isDirectory()) {
          const hit = e.name === session ? path.join(p, "session.jsonl") : find(p);
          if (hit && fs.existsSync(hit)) return hit;
        }
      }
      return null;
    };
    const root = path.join(home, ".local/share/muse/sessions");
    const file = fs.existsSync(root) ? find(root) : null;
    if (file) text = fs.readFileSync(file, "utf8");
  }
  const rules = [...text.matchAll(/<rules-file scope=\\"(\w+)\\" path=\\"([^"\\]+)\\"/g)].map((m) =>
    m[2].replace("$CONFIG_DIR", path.join(home, ".config/muse")).replace("$HOME", home),
  );
  return {
    skills: skills.map((s) => s.name),
    off: skills.filter((s) => s.activation === "off").map((s) => s.name),
    instructions: [...new Set(rules)],
    hooks: null,
    agents: null,
    gaps: ["Atlas writes no hooks for Muse, though its layout names ~/.config/muse/settings.json."],
    raw: `$ ${list.cmd}\n${list.out}${list.err}\n\n$ ${turn.cmd}\n${turn.out.slice(0, 4000)}${turn.err}\n\nrules files in the recorded prompt: ${JSON.stringify(rules)}`,
  };
}

async function grok({ home, cwd, trust }) {
  // Turning the folder-trust gate off for one run stands in for the owner trusting the folder.
  const r = await run(["grok", "inspect", "--json"], { home, cwd, env: trust ? { GROK_FOLDER_TRUST: "0" } : {} });
  const j = parse(r.out) ?? {};
  return {
    skills: (j.skills ?? []).map((s) => s.name),
    off: (j.skills ?? []).filter((s) => s.disabled).map((s) => s.name),
    instructions: (j.projectInstructions ?? []).map((p) => p.path),
    hooks: (j.hooks ?? []).map((h) => ({
      event: h.event,
      matcher: h.matcher,
      atlas: h.target?.match(/# atlas:(\S+)/)?.[1] ?? null,
      waitsForFolderTrust: h.waitsForFolderTrust,
    })),
    agents: (j.agents ?? []).map((a) => a.name),
    gaps: [],
    raw: `$ ${r.cmd}\n${r.out}${r.err}`,
  };
}

async function antigravity({ home, cwd }) {
  const r = await rpc(
    ["agy", "acp"],
    [
      ["initialize", { protocolVersion: 1, clientCapabilities: {} }],
      ["authenticate", { methodId: "gemini-api-key" }],
      ["session/new", { cwd, mcpServers: [] }],
    ],
    // The command list arrives as a notification shortly after session/new.
    { home, cwd, env: { GEMINI_API_KEY: "dummy" }, settle: 4000 },
  );
  const update = r.notes.find((n) => n.params?.update?.sessionUpdate === "available_commands_update");
  const commands = update?.params.update.availableCommands ?? [];
  return {
    skills: commands.map((c) => c.name),
    off: [],
    instructions: null,
    hooks: null,
    agents: null,
    gaps: [
      "Antigravity has no offline way to say which instruction files it loaded.",
      "Antigravity shows hooks only by running them, and offline only SessionStart runs; Atlas's no-rm is a pre-tool-use hook.",
      "No way known to switch a skill off in Antigravity, so a switched-off skill can't be checked there.",
    ],
    raw: `$ ${r.cmd}\n${r.out}${r.err}`,
  };
}

const PROBES = { "claude-code": claudeCode, codex, "open-code": openCode, pi, amp, muse, grok, antigravity };

/**
 * Asks harness `name` what it loaded for `home`, run in `cwd`. `marker` is a line of the
 * global instructions to look for; `check` names skills whose switch to test (Amp). `trust`
 * trusts `cwd` for this run only, as the owner would at the harness's trust prompt (Pi, Muse
 * and Grok load a project's skills only once it is trusted).
 */
export async function probe(name, { home, cwd, marker, check, trust = false }) {
  const bin = harnessBins()[HARNESSES[name].bin];
  if (!bin) return { installed: false, skills: [], off: [], instructions: null, hooks: null, agents: null, gaps: [`${HARNESSES[name].bin} isn't installed`], raw: "" };
  const result = await PROBES[name]({ home, cwd, marker, check, trust });
  return { installed: true, ...result };
}

/** Every harness at once, by agent id. */
export async function probeAll(opts) {
  const names = Object.keys(HARNESSES);
  const results = await Promise.all(names.map((n) => probe(n, opts)));
  return Object.fromEntries(names.map((n, i) => [n, results[i]]));
}

/** Whether `paths` (as a harness reported them) include the file `target` resolves to. */
export function readsFile(paths, target) {
  const want = real(target);
  return (paths ?? []).some((p) => real(p) === want);
}
