// Helpers for the distribution journeys: Send, pairing, installing atlasd and reading
// what it wrote on a fake machine. Everything a machine runs gets that machine's HOME.

import { execFileSync, spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { expect } from "@playwright/test";
import { BIN, git, sandboxEnv } from "./world.mjs";

/** The folder whose presence makes atlasd treat a harness as set up (Layout::marker). */
export const MARKERS = {
  claude: ".claude",
  codex: ".codex",
  opencode: ".config/opencode",
  pi: ".pi/agent",
  antigravity: ".gemini",
  amp: ".config/amp",
  muse: ".config/muse",
  grok: ".grok",
};

/** Makes the harness folders exist on a machine, as if those agents were installed. */
export function installHarnesses(m, names = Object.keys(MARKERS)) {
  for (const n of names) fs.mkdirSync(path.join(m.home, MARKERS[n]), { recursive: true });
}

/** Writes a file on a machine (relative to its HOME). */
export function put(m, rel, text) {
  const file = path.join(m.home, rel);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}

/**
 * Every file, folder and link under `dir`: path → content, `dir`, or `-> target`.
 * `skip` lists relative paths left out with everything under them.
 */
export function tree(dir, skip = []) {
  const out = {};
  const walk = (d) => {
    for (const e of fs.readdirSync(d, { withFileTypes: true })) {
      const p = path.join(d, e.name);
      const rel = path.relative(dir, p);
      if (skip.includes(rel)) continue;
      if (e.isSymbolicLink()) out[rel] = `-> ${fs.readlinkSync(p)}`;
      else if (e.isDirectory()) {
        out[rel] = "dir";
        walk(p);
      } else out[rel] = fs.readFileSync(p, "utf8");
    }
  };
  if (fs.existsSync(dir)) walk(dir);
  return out;
}

/** Commits files in a repo by hand, as the owner editing plain files would. */
export function commitFiles(world, dir, files, message) {
  for (const [rel, text] of Object.entries(files)) {
    const file = path.join(dir, rel);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
  git(world, dir, "add", "-A");
  git(world, dir, "commit", "-q", "-m", message);
  return git(world, dir, "rev-parse", "HEAD");
}

/** The commit a bare remote's main points at. */
export const remoteHead = (world, name) => git(world, path.join(world.remotes, `${name}.git`), "rev-parse", "main");

// ---------------------------------------------------------------- the owner's side

/** Clicks Pair on the Machines page and returns the command shown and its code. */
export async function pairCode(owner, { machine } = {}) {
  await owner.goto("/machines");
  if (machine) {
    const ledger = owner.locator("section.ledger", { has: owner.getByRole("heading", { name: machine, exact: true }) });
    await ledger.getByRole("button", { name: "Pair", exact: true }).click();
  } else {
    await owner.locator("form.setting-row[action='/machines/pair']").getByRole("button", { name: "Pair" }).click();
  }
  await expect(owner.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
  const command = (await owner.locator("pre.ink-block").first().textContent()).trim();
  const code = command.split(" ").pop();
  return { command, code };
}

/** How many updates Settings lists. */
export async function updateCount(owner) {
  await owner.goto("/settings");
  return Number(await owner.locator("#updates .section-head .count").textContent());
}

/** Presses Send now in Settings (one passkey tap) and waits for the page it leads to. */
export async function sendNow(owner) {
  await owner.goto("/settings");
  await owner.locator("#updates").getByRole("button", { name: "Send now" }).click();
  // Sent (through "Check before sending" when hooks changed), or nothing new to send.
  await Promise.race([
    owner.waitForURL(/flash=sent/),
    owner.locator("#updates").getByText(NOTHING_NEW).waitFor(),
  ]);
}

/** What Send says when the machines already have everything (decision 0040's branch). */
export const NOTHING_NEW = "Your machines already have everything.";

// ---------------------------------------------------------------- the machine's side

/** Puts the debug atlasd where the hub hands builds out, as the owner would after a build. */
export function publishBuild(world) {
  const arch = { x64: "x86_64", arm64: "aarch64" }[os.arch()] ?? os.arch();
  const dist = path.join(world.state, "dist");
  fs.mkdirSync(dist, { recursive: true });
  const file = path.join(dist, `atlasd-linux-${arch}`);
  fs.copyFileSync(path.join(BIN, "atlasd"), file);
  return { arch, file };
}

/**
 * Writes an executable without this process ever holding it open for writing, so a fork
 * elsewhere can't inherit the handle and make exec fail with ETXTBSY (see commit 9ff1304).
 */
export function writeExecutable(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  execFileSync("sh", ["-c", 'cat > "$1" && chmod 755 "$1"', "sh", file], { input: text });
}

/**
 * Fake machine tools in `<root>/fakebin`: `machine/systemctl` and `machine/loginctl` record
 * their arguments and do nothing (so no install can reach the real systemd user session),
 * and `ssh` runs its command locally as the machine named by the target, in that
 * machine's HOME under `<root>/machines/<host>`, with the shims first on PATH.
 */
export function writeFakeBin(root) {
  const bin = path.join(root, "fakebin");
  const shims = path.join(bin, "machine");
  for (const tool of ["systemctl", "loginctl"])
    writeExecutable(path.join(shims, tool), `#!/bin/sh\nprintf '%s\\n' "${tool} $*" >> "$HOME/.fake-${tool}.log"\nexit 0\n`);
  writeExecutable(
    path.join(bin, "ssh"),
    `#!/bin/sh
# Fake ssh: logs its arguments, then runs the last one as the machine after --.
printf '%s\\n' "$*" >> "${root}/ssh.log"
target=""; seen=""
for a; do
  if [ -n "$seen" ] && [ -z "$target" ]; then target="$a"; fi
  [ "$a" = "--" ] && seen=1
  cmd="$a"
done
host="\${target#*@}"
home="${root}/machines/$host"
[ -d "$home" ] || { echo "ssh: Could not resolve hostname $host: Name or service not known" >&2; exit 255; }
exec "${shims}/../run-as" "$host" "$home" "$cmd"
`,
  );
  // Runs a command as a machine: its HOME, its name (a private UTS namespace) and the shims.
  writeExecutable(
    path.join(bin, "run-as"),
    `#!/bin/sh
host="$1"; home="$2"; cmd="$3"
exec env -i PATH="${shims}:/usr/local/bin:/usr/bin:/bin" HOME="$home" USER=verify LANG=C.UTF-8 \\
  XDG_CONFIG_HOME="$home/.config" XDG_DATA_HOME="$home/.local/share" XDG_STATE_HOME="$home/.local/state" \\
  XDG_CACHE_HOME="$home/.cache" GIT_CONFIG_GLOBAL="$home/.gitconfig" GIT_CONFIG_NOSYSTEM=1 \\
  unshare -r -u sh -c 'hostname "$1" && exec sh -c "$2"' sh "$host" "$cmd"
`,
  );
  return { bin, shims, ssh: path.join(bin, "ssh"), runAs: path.join(bin, "run-as"), log: path.join(root, "ssh.log") };
}

/**
 * Runs a program without blocking: the proxy in front of the hub lives in this process,
 * so a synchronous child that talks to the hub (as `machine().atlasd` does) would wait
 * on a proxy that can't answer. Resolves to `{ code, out }` with stdout and stderr.
 */
export function run(program, args, { env, timeout = 90000 } = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(program, args, { env, stdio: ["ignore", "pipe", "pipe"] });
    let out = "";
    child.stdout.on("data", (d) => (out += d));
    child.stderr.on("data", (d) => (out += d));
    const timer = setTimeout(() => child.kill("SIGKILL"), timeout);
    child.on("error", reject);
    child.on("close", (code) => {
      clearTimeout(timer);
      resolve({ code, out });
    });
  });
}

/** `atlasd ...args` as machine `m`; `path` puts extra folders first on its PATH. */
export function atlasd(m, args, { path: extra } = {}) {
  const env = sandboxEnv(m.home, extra ? { PATH: `${extra}:${process.env.PATH}` } : {});
  return run(path.join(BIN, "atlasd"), args, { env });
}

/** Runs a shell command as machine `m` (its HOME, its hostname, systemctl shimmed). */
export const runAsMachine = (fake, m, command) => run(fake.runAs, [m.name, m.home, command]);

/** `atlas-hub ...args` as the hub's user, with the world's extra environment. */
export const hubCommand = (world, ...args) =>
  run(path.join(BIN, "atlas-hub"), args, {
    env: sandboxEnv(world.home, { ...world.env, ATLAS_HUB_CONFIG: world.config }),
    timeout: 240000,
  });

/** Pairs machine `m` with a fresh code from the Machines page. */
export async function pairMachine(owner, world, m) {
  const { code } = await pairCode(owner);
  const r = await atlasd(m, ["pair", world.url, code, "--machine", m.name]);
  expect(r.code, r.out).toBe(0);
  return r;
}

/** A machine's harness files: everything but atlasd's own copies and state. */
export const harnessFiles = (m) => tree(m.home, [".local/share/atlas", ".local/state/atlas-client"]);

/** The local copy of a source on a machine. */
export const copy = (m, rel) => path.join(m.home, ".local/share/atlas/sources", rel);
