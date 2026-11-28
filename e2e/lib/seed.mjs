// The seed store every world starts from: enough of each kind of item that every screen
// has something real to show. Each repo is pushed to a local bare remote, so Send can push
// and atlasd can fetch exactly as with GitHub.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

function write(file, text) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, text);
}

function repo(world, dir, files) {
  const env = { ...process.env, HOME: world.home, GIT_CONFIG_GLOBAL: path.join(world.home, ".gitconfig"), GIT_CONFIG_NOSYSTEM: "1" };
  const run = (cwd, ...args) => execFileSync("git", args, { cwd, env, stdio: "pipe" });
  for (const [rel, text] of Object.entries(files)) {
    write(path.join(dir, rel), text);
    // Scripts are committed executable, as an owner's working setup is (decision 0040).
    if (rel.endsWith(".sh")) fs.chmodSync(path.join(dir, rel), 0o755);
  }
  run(dir, "init", "-q", "-b", "main");
  run(dir, "add", "-A");
  run(dir, "commit", "-q", "-m", "Seed");
  const bare = path.join(world.remotes, `${path.basename(dir)}.git`);
  fs.mkdirSync(world.remotes, { recursive: true });
  run(world.remotes, "init", "-q", "--bare", "-b", "main", bare);
  run(dir, "remote", "add", "origin", bare);
  run(dir, "push", "-q", "-u", "origin", "main");
}

const skill = (name, description, body = "") =>
  `---\nname: ${name}\ndescription: ${description}\n---\n# ${name}\n\n${body || `Use ${name} when asked.`}\n`;

const memory = (name, description, type, host, body) =>
  `---\nname: ${name}\ndescription: ${description}\nmetadata:\n  type: ${type}\n  host: ${host}\n---\n${body}\n`;

export function seed(world) {
  repo(world, world.knowledge, {
    "AGENTS.md": "# Global agent instructions\n\nAlways use the simplest solution that works.\n",
    "skills/deploy/SKILL.md": skill("deploy", "Ship the app to production", "Run the deploy script.\n"),
    "skills/deploy/scripts/run.sh": "#!/bin/sh\necho deploy\n",
    "skills/pdf-tools/SKILL.md": skill("pdf-tools", "Read and merge PDF files"),
    "memory/index.md": "# Memory index\n\n- [[user-tz]]\n- [[verify-outcomes]]\n",
    "memory/user-tz.md": memory("user-tz", "The owner's time zone", "user", "all", "Eastern. See [[verify-outcomes]]."),
    "memory/verify-outcomes.md": memory(
      "verify-outcomes",
      "Verify outcomes, not status",
      "feedback",
      "all",
      "Confirm what the user sees before claiming a fix worked.\n\n**Why:** status lies.",
    ),
    "memory/hosts/laptop/disk.md": memory("disk", "Laptop disk layout", "reference", "laptop", "One NVMe drive."),
    "hooks/no-rm/hook.toml":
      'description = "Block rm -rf"\nevent = "pre-tool-use"\nmatcher = "Bash"\nrun = "check.sh"\ntimeout = 10\n',
    "hooks/no-rm/check.sh": "#!/bin/sh\nexit 0\n",
    "agents/reviewer.md": "---\nname: reviewer\ndescription: Reviews a diff for bugs\n---\nReview the diff.\n",
  });
  repo(world, world.publicSkills, {
    "README.md": "# Public skills\n",
    "review/SKILL.md": skill("review", "Review code for correctness"),
  });
  repo(world, path.join(world.projectsDir, "shop"), {
    "AGENTS.md": "# Shop\n\nRun the tests before committing.\n",
    ".agents/skills/stock/SKILL.md": skill("stock", "Count stock in the shop database"),
  });
}
