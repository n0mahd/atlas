// The hub refuses to serve when the folder holding its socket is open to other users
// (check_private in crates/atlas-hub/src/main.rs): the socket is the trust boundary,
// since anything reaching it can claim any Tailscale login.

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { test, expect } from "../../lib/owner.mjs";
import { BIN, sandboxEnv, startHub, stopHub } from "../../lib/world.mjs";
import { request } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "socket" }, enrol: false });

test("an open socket folder stops the hub from starting", async ({ world }) => {
  await stopHub(world);
  const config = fs.readFileSync(world.config, "utf8");
  try {
    for (const mode of [0o755, 0o770, 0o701, 0o707]) {
      const dir = fs.mkdtempSync(path.join(world.runtime, "o-"));
      fs.chmodSync(dir, mode);
      const alt = path.join(world.root, `hub-${mode.toString(8)}.toml`);
      fs.writeFileSync(alt, config.replace(/^socket = .*$/m, `socket = "${dir}/hub.sock"`));
      const r = spawnSync(path.join(BIN, "atlas-hub"), [], {
        env: sandboxEnv(world.home, { ATLAS_HUB_CONFIG: alt }),
        encoding: "utf8",
        timeout: 15000,
      });
      expect(r.status, `mode ${mode.toString(8)}: ${r.stderr}`).toBe(1);
      expect(r.stderr).toContain("must belong to this user and be closed to others (chmod 700)");
    }
  } finally {
    // Delete the stale socket first, or startHub returns before the new hub listens.
    fs.rmSync(world.socket, { force: true });
    await startHub(world);
  }
  // The real one, in a 0700 folder, serves again.
  expect((await request(world, { path: "/" })).status).toBe(200);
});
