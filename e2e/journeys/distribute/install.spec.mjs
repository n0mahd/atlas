// The one-line install: `curl <hub>/install | sh -s -- <code>`. The hub serves a script with
// each build's checksum and the builds it keeps in <state>/dist/. The script is run for
// real on a fake machine: its own HOME and hostname, with systemctl and loginctl replaced
// by shims that only record their arguments, so nothing reaches the real systemd session.

import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { test, expect, shot } from "../../lib/owner.mjs";
import { REPO, machine, rawRequest } from "../../lib/world.mjs";
import { atlasd, installHarnesses, pairCode, publishBuild, runAsMachine, sendNow, writeFakeBin } from "../../lib/distribute.mjs";

test.use({ worldOptions: { tag: "install" } });

const sha256 = (bytes) => crypto.createHash("sha256").update(bytes).digest("hex");


test("the script carries the hub's address and the build's checksum", async ({ world }) => {
  const { arch, file } = publishBuild(world);
  const res = await fetch(`${world.url}/install`);
  expect(res.status).toBe(200);
  expect(res.headers.get("content-type")).toContain("text/x-shellscript");
  const script = await res.text();
  test.info().attach("install script", { body: script, contentType: "text/plain" });
  expect(script).toContain(`hub="${world.url}"`);
  expect(script).toContain(`# Usage: curl -fsSL ${world.url}/install | sh -s -- <pairing code>`);
  expect(script).toContain(`  linux-${arch}) sum=${sha256(fs.readFileSync(file))} ;;`);
  // What the script does to the session, so the run below can shim it.
  expect(script).toContain("systemctl --user enable --now atlasd.timer");
  expect(script).toContain("loginctl enable-linger");
  expect(script).toContain(fs.readFileSync(path.join(REPO, "deploy/atlasd.service"), "utf8"));

  const bin = await fetch(`${world.url}/install/atlasd-linux-${arch}`);
  expect(bin.status).toBe(200);
  expect(sha256(Buffer.from(await bin.arrayBuffer()))).toBe(sha256(fs.readFileSync(file)));
  expect((await fetch(`${world.url}/install/atlasd-linux-sparc`)).status).toBe(404);
  expect((await fetch(`${world.url}/install/passkeys.json`)).status).toBe(404);
  expect((await rawRequest(world, { path: "/install" })).status, "only the owner's tailnet login").toBe(403);
});

test("running the script installs, pairs and sets up the timer on a fresh machine", async ({ owner, world }) => {
  publishBuild(world);
  const fake = writeFakeBin(world.root);
  const m = machine(world, "fresh");
  installHarnesses(m, ["claude", "codex"]);
  fs.mkdirSync(path.join(m.home, "Knowledge")); // an older setup's folder, offered for adoption
  await sendNow(owner);

  // A code that doesn't exist: the script stops at pairing and says why.
  const bad = await runAsMachine(fake, m, `curl -fsSL ${world.url}/install | sh -s -- not-a-code`);
  expect(bad.code).not.toBe(0);
  expect(bad.out).toContain("expired or was already used");
  expect(m.exists(".config/atlas/client.toml")).toBe(false);

  const { command } = await pairCode(owner);
  const r = await runAsMachine(fake, m, command);
  test.info().attach("install output", { body: r.out });
  expect(r.code, r.out).toBe(0);
  expect(r.out).toContain("Paired fresh (verify)");
  // Decision 0041: the script runs the first pass itself, with a short summary, then says Done.
  expect(r.out).toContain("Done. atlasd checks for updates every minute (log: journalctl --user -u atlasd).");
  expect(r.out.indexOf("Paired fresh")).toBeLessThan(r.out.indexOf("Done."));
  expect(m.link(".claude/skills/deploy"), "the first pass ran before Done").toBe(path.join(m.home, ".local/share/atlas/sources/knowledge/skills/deploy"));

  const installed = path.join(m.home, ".local/bin/atlasd");
  expect(fs.statSync(installed).mode & 0o777).toBe(0o755);
  expect(sha256(fs.readFileSync(installed))).toBe(sha256(fs.readFileSync(path.join(REPO, "target/debug/atlasd"))));
  const config = m.read(".config/atlas/client.toml");
  expect(config).toContain('machine = "fresh"');
  expect(config).toContain(`hub = "${world.url}"`);
  // 0041: the script adopts no folder on its own (atlasd pair --adopt stays for the owner).
  expect(config).not.toContain(path.join(m.home, "Knowledge"));
  for (const unit of ["atlasd.service", "atlasd.timer"])
    expect(m.read(`.config/systemd/user/${unit}`)).toBe(fs.readFileSync(path.join(REPO, "deploy", unit), "utf8"));
  // 0041: checks for systemd user services before downloading, then sets up the timer;
  // the first pass already ran in the foreground, so nothing starts the service.
  const systemctl = m.read(".fake-systemctl.log").trim().split("\n");
  expect(systemctl[0]).toBe("systemctl --user show-environment");
  expect(systemctl.slice(-2)).toEqual(["systemctl --user daemon-reload", "systemctl --user enable --now atlasd.timer"]);
  expect(systemctl.filter((l) => / start /.test(l)), "nothing starts the service").toEqual([]);
  expect(m.read(".fake-loginctl.log").trim()).toBe("loginctl enable-linger verify");

  // The shimmed service never ran, so run what it would run.
  const run = await runAsMachine(fake, m, "$HOME/.local/bin/atlasd run");
  expect(run.code, run.out).toBe(0);
  expect(m.link(".claude/skills/deploy")).toBe(path.join(m.home, ".local/share/atlas/sources/knowledge/skills/deploy"));
  await owner.goto("/machines");
  const ledger = owner.locator("section.ledger", { has: owner.getByRole("heading", { name: "fresh", exact: true }) });
  await expect(ledger.locator(".client-row")).toContainText("verify");
  await expect(ledger.locator(".client-row")).toContainText("up to date");
  await shot(owner, "machines: fresh installed by the script");
  expect((await atlasd(m, ["status"])).out).toContain("Machine: fresh");
});
