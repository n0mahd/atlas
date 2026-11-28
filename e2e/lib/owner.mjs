// The owner in a real browser: a Chromium context with a virtual passkey authenticator
// (CDP WebAuthn), so enrolment, sign-in and signing an update run the real ceremonies.
// Also the evidence helpers every journey uses.

import { test as base, expect } from "@playwright/test";
import { createWorld, destroyWorld } from "./world.mjs";

/** Adds a virtual authenticator to `page`, optionally holding credentials from another. */
export async function addAuthenticator(page, credentials = []) {
  const cdp = await page.context().newCDPSession(page);
  await cdp.send("WebAuthn.enable", { enableUI: false });
  const { authenticatorId } = await cdp.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });
  for (const credential of credentials)
    await cdp.send("WebAuthn.addCredential", { authenticatorId, credential });
  // Methods read `this.authenticatorId`, which a journey may replace (swapAuthenticator).
  return {
    cdp,
    authenticatorId,
    async credentials() {
      return (await cdp.send("WebAuthn.getCredentials", { authenticatorId: this.authenticatorId })).credentials;
    },
    /** Makes the next ceremony fail as if the owner cancelled the prompt. */
    refuse(on = true) {
      return cdp.send("WebAuthn.setUserVerified", { authenticatorId: this.authenticatorId, isUserVerified: !on });
    },
  };
}

/** Collects console errors, page errors and CSP violations; journeys assert none. */
export function watchConsole(page) {
  const problems = [];
  page.on("console", (m) => {
    if (m.type() === "error") problems.push(`console: ${m.text()}`);
  });
  page.on("pageerror", (e) => problems.push(`pageerror: ${e.message}`));
  return problems;
}

const allowed = new WeakMap();

/**
 * Lets the current test expect console lines matching `patterns` on `page`, such as the
 * "Failed to load resource ... 409" Chrome logs when the hub answers a refused save.
 */
export function allowConsole(page, ...patterns) {
  allowed.set(page, [...(allowed.get(page) ?? []), ...patterns]);
}

/** A named full-page screenshot attached to the test report: one per meaningful state. */
export async function shot(page, label) {
  const info = base.info();
  const n = String(info.attachments.filter((a) => a.name.startsWith("step ")).length + 1).padStart(2, "0");
  const slug = label.replace(/[^a-z0-9]+/gi, "-").replace(/^-|-$/g, "").toLowerCase() || "root";
  const file = info.outputPath(`${n}-${slug}.png`);
  await page.screenshot({ path: file, fullPage: true });
  await info.attach(`step ${n}: ${label}`, { path: file, contentType: "image/png" });
}

/**
 * Decision 0040: an update that adds or changes hooks (or trusted passkeys) first shows
 * "Check before sending". By default the owner looks at it (a screenshot in the report)
 * and confirms with Send, so journeys about something else go through it as the owner
 * would. Journeys about the page itself call `manualReview(page)` and drive it by hand.
 */
export function autoReview(page) {
  page.on("load", async () => {
    if (page.__manualReview) return;
    let url;
    try {
      url = new URL(page.url());
    } catch {
      return;
    }
    if (!url.pathname.startsWith("/send/review/")) return;
    try {
      await shot(page, "check before sending");
      await page.locator("section[data-pending] [data-action='send']").click();
    } catch {
      // The page moved on (closed, or the journey drove it); nothing to confirm.
    }
  });
}

/** Lets a journey drive "Check before sending" itself. */
export function manualReview(page, on = true) {
  page.__manualReview = on;
}

/** Enrols the first passkey through the first-run screen. */
export async function enrolFirstRun(page, world, name = "Verify laptop") {
  await page.goto(`${world.url}/signin`);
  await page.getByPlaceholder(/Laptop, phone/).fill(name);
  await page.getByRole("button", { name: "Create passkey" }).click();
  await page.waitForURL((u) => !u.pathname.startsWith("/signin"));
}

/** Signs in with the passkey the authenticator already holds. */
export async function signIn(page, world, next = "/") {
  await page.goto(`${world.url}/signin?next=${encodeURIComponent(next)}`);
  await page.getByRole("button", { name: /Sign in with a passkey/ }).click();
  await page.waitForURL((u) => !u.pathname.startsWith("/signin"));
}

/**
 * Test with one world per spec file (serial), and `owner`: a page signed in with a fresh
 * passkey. Use `test.use({ worldOptions: {...} })` to change how the world starts and
 * `test.use({ enrol: false })` to start signed out with no passkey.
 */
export const test = base.extend({
  worldOptions: [{}, { option: true, scope: "worker" }],
  enrol: [true, { option: true, scope: "worker" }],
  world: [
    async ({ worldOptions }, use, workerInfo) => {
      const w = await createWorld(`${workerInfo.project.name}-w${workerInfo.workerIndex}`, worldOptions);
      await use(w);
      await destroyWorld(w);
    },
    { scope: "worker", timeout: 60000 },
  ],
  // One browser context and passkey per worker, so journeys in a file share a session.
  ownerSession: [
    async ({ browser, world, enrol }, use, workerInfo) => {
      const context = await browser.newContext({
        ...workerInfo.project.use,
        baseURL: world.url,
        recordVideo: { dir: `${world.root}/video` },
      });
      const page = await context.newPage();
      autoReview(page);
      const problems = watchConsole(page);
      const auth = await addAuthenticator(page);
      if (enrol) await enrolFirstRun(page, world);
      await use({ context, page, auth, problems });
      await context.close();
    },
    { scope: "worker", timeout: 60000 },
  ],
  owner: async ({ ownerSession }, use) => {
    ownerSession.problems.length = 0;
    await use(ownerSession.page);
    const ok = allowed.get(ownerSession.page) ?? [];
    allowed.delete(ownerSession.page);
    const problems = ownerSession.problems.filter((p) => !ok.some((r) => r.test(p)));
    expect(problems, "no console errors or CSP violations").toEqual([]);
  },
  auth: async ({ ownerSession }, use) => use(ownerSession.auth),
});

export { expect };
