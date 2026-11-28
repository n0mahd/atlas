// Every journey runs at desktop and phone width. Evidence (HTML report with a screenshot
// per step, video, trace, and each world's files and hub log) goes to target/verify/<run>.

import { defineConfig, devices } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";

const repo = path.resolve(import.meta.dirname, "..");
if (!process.env.ATLAS_VERIFY_RUN) {
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  process.env.ATLAS_VERIFY_RUN = path.join(repo, "target/verify", stamp);
  fs.mkdirSync(process.env.ATLAS_VERIFY_RUN, { recursive: true });
  const latest = path.join(repo, "target/verify/latest");
  fs.rmSync(latest, { force: true });
  fs.symlinkSync(process.env.ATLAS_VERIFY_RUN, latest);
}
const run = process.env.ATLAS_VERIFY_RUN;

const chromium = { browserName: "chromium", launchOptions: { executablePath: process.env.ATLAS_VERIFY_CHROMIUM } };

export default defineConfig({
  testDir: ".",
  // Timings mean nothing next to other workers, so perf runs on its own (npm run perf).
  testMatch: process.env.ATLAS_VERIFY_PERF ? ["journeys/perf/**/*.spec.mjs"] : ["journeys/**/*.spec.mjs"],
  testIgnore: process.env.ATLAS_VERIFY_PERF ? [] : ["journeys/perf/**"],
  globalSetup: "./lib/global-setup.mjs",
  outputDir: path.join(run, "results"),
  timeout: 60000,
  expect: { timeout: 10000, toHaveScreenshot: { maxDiffPixelRatio: 0, animations: "disabled" } },
  snapshotPathTemplate: "{testDir}/baselines/{projectName}/{testFilePath}/{arg}{ext}",
  fullyParallel: false,
  workers: Number(process.env.ATLAS_VERIFY_WORKERS ?? 4),
  reporter: [
    ["list"],
    ["html", { outputFolder: path.join(run, "report"), open: "never" }],
    ["json", { outputFile: path.join(run, "results.json") }],
  ],
  use: { trace: "on", screenshot: "on", video: "on", ...chromium },
  projects: [
    { name: "desktop", use: { ...devices["Desktop Chrome"], ...chromium, viewport: { width: 1366, height: 900 } } },
    { name: "phone", use: { ...devices["Pixel 7"], ...chromium } },
  ],
});
