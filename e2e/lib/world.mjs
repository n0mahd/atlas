// A throwaway Atlas: seeded git repos with local bare remotes, a hub on its own socket,
// and a proxy in front that adds the Tailscale identity the way `tailscale serve` does.
// Nothing here reads or writes the real home folder: every process gets HOME, XDG_* and
// GIT_CONFIG_GLOBAL inside the world.

import { spawn, execFileSync } from "node:child_process";
import fs from "node:fs";
import http from "node:http";
import net from "node:net";
import path from "node:path";
import { seed } from "./seed.mjs";

export const REPO = path.resolve(import.meta.dirname, "../..");
export const RUN_DIR = process.env.ATLAS_VERIFY_RUN ?? path.join(REPO, "target/verify/dev");
export const BIN = path.join(REPO, "target/debug");
export const OWNER = "owner@github";

/** Builds the two binaries once per run (globalSetup). */
export function buildBinaries() {
  execFileSync("cargo", ["build", "-q", "-p", "atlas-hub", "-p", "atlas-client"], {
    cwd: REPO,
    stdio: "inherit",
  });
}

function freePort() {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address();
      s.close(() => resolve(port));
    });
    s.on("error", reject);
  });
}

async function waitFor(check, what, ms = 15000) {
  const end = Date.now() + ms;
  for (;;) {
    if (await check()) return;
    if (Date.now() > end) throw new Error(`timed out waiting for ${what}`);
    await new Promise((r) => setTimeout(r, 100));
  }
}

/** Environment for any process that belongs to `home`. */
export function sandboxEnv(home, extra = {}) {
  return {
    PATH: process.env.PATH,
    LANG: "C.UTF-8",
    HOME: home,
    XDG_CONFIG_HOME: path.join(home, ".config"),
    XDG_DATA_HOME: path.join(home, ".local/share"),
    XDG_STATE_HOME: path.join(home, ".local/state"),
    XDG_CACHE_HOME: path.join(home, ".cache"),
    GIT_CONFIG_GLOBAL: path.join(home, ".gitconfig"),
    GIT_CONFIG_NOSYSTEM: "1",
    GIT_TERMINAL_PROMPT: "0",
    ...extra,
  };
}

export function git(world, dir, ...args) {
  return execFileSync("git", args, {
    cwd: dir,
    env: sandboxEnv(world.home),
    encoding: "utf8",
  }).trim();
}

/**
 * Starts a world. `name` labels its folder under the run directory, which survives
 * cleanup as evidence. Options: `login` (identity the proxy adds; null sends none),
 * `firstRunMinutes`, `readOnly` (no public_url), `seedWith` (extra seeding callback),
 * `env` (extra environment for the hub and `hubCli`: an object, or a function of the world,
 * such as a PATH with a fake `ssh` first).
 */
export async function createWorld(name, opts = {}) {
  const root = path.join(RUN_DIR, "worlds", `${name}-${process.pid}-${Date.now()}`);
  const home = path.join(root, "home");
  fs.mkdirSync(home, { recursive: true });
  fs.writeFileSync(
    path.join(home, ".gitconfig"),
    "[user]\n\tname = Atlas Verify\n\temail = verify@example.invalid\n[init]\n\tdefaultBranch = main\n",
  );
  const world = {
    name,
    root,
    home,
    knowledge: path.join(home, "Knowledge"),
    publicSkills: path.join(home, "Skills"),
    projectsDir: path.join(home, "Projects"),
    remotes: path.join(root, "remotes"),
    state: path.join(home, ".local/state/atlas-hub"),
    procs: [],
    servers: [],
    login: opts.login === undefined ? OWNER : opts.login,
  };
  seed(world);
  if (opts.seedWith) opts.seedWith(world);
  // Decision 0041: no pairing codes without a build to hand out, so a world has one, as an
  // owner who followed the hub guide would. `noBuild` starts without.
  if (!opts.noBuild) publishDebugBuild(world);
  world.env = typeof opts.env === "function" ? opts.env(world) : (opts.env ?? {});

  // Unix socket paths are limited to 108 bytes, so the socket lives in a short folder.
  const runtime = fs.mkdtempSync(path.join(process.env.XDG_RUNTIME_DIR ?? "/tmp", "av-"));
  fs.chmodSync(runtime, 0o700);
  world.runtime = runtime;
  world.socket = path.join(runtime, "hub.sock");
  world.port = await freePort();
  world.url = `http://localhost:${world.port}`;
  world.config = path.join(root, "hub.toml");
  const lines = [
    `allowed_logins = ["${OWNER}"]`,
    `knowledge = "~/Knowledge"`,
    `public_skills = "~/Skills"`,
    `projects_dir = "~/Projects"`,
    `machines = ["laptop"]`,
    `socket = "${world.socket}"`,
    `hub_name = "verify-hub"`,
    `first_run_minutes = ${opts.firstRunMinutes ?? 30}`,
  ];
  if (!opts.readOnly) lines.push(`public_url = "${world.url}"`);
  fs.writeFileSync(world.config, lines.join("\n") + "\n");

  world.hubLog = path.join(root, "hub.log");
  await startHub(world);
  world.servers.push(await startProxy(world));
  return world;
}

/** Puts the debug atlasd where the hub hands builds out (`<state>/dist/`). */
export function publishDebugBuild(world) {
  const arch = { x64: "x86_64", arm64: "aarch64" }[process.arch] ?? process.arch;
  const dist = path.join(world.state, "dist");
  // The hub keeps its state closed to others (0700); create it that way before it does.
  fs.mkdirSync(dist, { recursive: true, mode: 0o700 });
  const file = path.join(dist, `atlasd-linux-${arch}`);
  fs.copyFileSync(path.join(BIN, "atlasd"), file);
  return { arch, file };
}

/** Starts (or restarts) the hub process for `world`. */
export async function startHub(world) {
  // A stopped hub leaves its socket file behind; waiting for it would pass at once.
  fs.rmSync(world.socket, { force: true });
  const log = fs.openSync(world.hubLog, "a");
  const hub = spawn(path.join(BIN, "atlas-hub"), [], {
    env: sandboxEnv(world.home, {
      ...world.env,
      ATLAS_HUB_CONFIG: world.config,
      RUST_LOG: "info",
    }),
    stdio: ["ignore", log, log],
  });
  world.hub = hub;
  world.procs.push(hub);
  // So an aborted run's hub can be stopped by PID, never by name.
  fs.writeFileSync(path.join(world.root, "hub.pid"), `${hub.pid}\n`);
  let exited = null;
  hub.on("exit", (code) => (exited = code));
  await waitFor(
    () => {
      if (exited !== null)
        throw new Error(`atlas-hub exited (${exited}):\n${fs.readFileSync(world.hubLog, "utf8")}`);
      return fs.existsSync(world.socket);
    },
    "the hub socket",
  );
}

/** Stops the hub only (for "hub is down" journeys). */
export async function stopHub(world) {
  const hub = world.hub;
  if (!hub || hub.exitCode !== null) return;
  await new Promise((resolve) => {
    hub.once("exit", resolve);
    hub.kill("SIGTERM");
  });
}

function startProxy(world) {
  const server = http.createServer((req, res) => {
    const headers = { ...req.headers };
    // tailscale serve replaces any identity the client sent with the real one.
    delete headers["tailscale-user-login"];
    if (world.login) headers["tailscale-user-login"] = world.login;
    const up = http.request(
      { socketPath: world.socket, path: req.url, method: req.method, headers },
      (r) => {
        res.writeHead(r.statusCode, r.headers);
        r.pipe(res);
      },
    );
    up.on("error", () => {
      if (!res.headersSent) res.writeHead(502, { "content-type": "text/plain" });
      res.end("hub unreachable");
    });
    req.pipe(up);
  });
  return new Promise((resolve) => server.listen(world.port, "127.0.0.1", () => resolve(server)));
}

/** A raw request straight to the hub socket, bypassing the proxy (no identity added). */
export function rawRequest(world, { method = "GET", path: p = "/", headers = {}, body } = {}) {
  return new Promise((resolve, reject) => {
    const req = http.request({ socketPath: world.socket, path: p, method, headers }, (res) => {
      let data = "";
      res.on("data", (c) => (data += c));
      res.on("end", () => resolve({ status: res.statusCode, headers: res.headers, body: data }));
    });
    req.on("error", reject);
    if (body) req.write(body);
    req.end();
  });
}

/** Runs an `atlas-hub` subcommand (setup, pair-code, token) as the hub's user. */
export function hubCli(world, ...args) {
  return execFileSync(path.join(BIN, "atlas-hub"), args, {
    env: sandboxEnv(world.home, { ...world.env, ATLAS_HUB_CONFIG: world.config }),
    encoding: "utf8",
  });
}

/** A client machine: its own fake HOME, where atlasd runs and harness folders live. */
export function machine(world, name) {
  const home = path.join(world.root, "machines", name);
  fs.mkdirSync(home, { recursive: true });
  fs.writeFileSync(
    path.join(home, ".gitconfig"),
    "[user]\n\tname = Atlas Verify\n\temail = verify@example.invalid\n",
  );
  const m = {
    name,
    home,
    // Async on purpose: atlasd talks to the hub through the proxy in this process, so a
    // synchronous child would deadlock it.
    atlasd(...args) {
      return new Promise((resolve, reject) => {
        const child = spawn(path.join(BIN, "atlasd"), args, {
          env: sandboxEnv(home),
          stdio: ["ignore", "pipe", "pipe"],
        });
        let out = "";
        child.stdout.on("data", (d) => (out += d));
        child.stderr.on("data", (d) => (out += d));
        const timer = setTimeout(() => child.kill("SIGKILL"), 90000);
        child.on("error", reject);
        child.on("close", (code) => {
          clearTimeout(timer);
          resolve({ code, out });
        });
      });
    },
    read: (rel) => fs.readFileSync(path.join(home, rel), "utf8"),
    exists: (rel) => fs.existsSync(path.join(home, rel)),
    link: (rel) => {
      try {
        return fs.readlinkSync(path.join(home, rel));
      } catch {
        return null;
      }
    },
  };
  return m;
}

/** Kills what this world started. Keeps every file: they are the evidence. */
export async function destroyWorld(world) {
  if (!world) return;
  for (const s of world.servers) await new Promise((r) => s.close(r));
  for (const p of world.procs) {
    if (p.exitCode === null) {
      await new Promise((resolve) => {
        p.once("exit", resolve);
        p.kill("SIGTERM");
        setTimeout(() => p.kill("SIGKILL"), 3000).unref();
      });
    }
  }
  fs.rmSync(world.runtime, { recursive: true, force: true });
}
