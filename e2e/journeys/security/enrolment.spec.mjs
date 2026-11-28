// Passkey enrolment: the first passkey comes from the allowed Tailscale login during the
// first-run window, once. After that, another passkey needs a session or a one-time
// setup link from `atlas-hub setup` on the hub. Attacker: another device on the owner's
// tailnet (so the login passes) with no session, trying to add its own passkey.

import { swapAuthenticator } from "../../lib/catalog.mjs";
import { test, expect, shot, addAuthenticator, enrolFirstRun } from "../../lib/owner.mjs";
import { hubCli } from "../../lib/world.mjs";
import { ownerSecrets, request } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "enrolment" }, enrol: false });
test.describe.configure({ mode: "serial" });

const enrolStart = (world, body, headers = {}) =>
  request(world, {
    method: "POST",
    path: "/auth/enrol/start",
    headers: { "content-type": "application/json", origin: world.url, ...headers },
    body: JSON.stringify(body),
  });

/** A second device: a fresh browser context with its own authenticator and no cookie. */
async function otherDevice(browser, info, world) {
  const context = await browser.newContext({ ...info.project.use, baseURL: world.url });
  const page = await context.newPage();
  const auth = await addAuthenticator(page);
  return { context, page, auth };
}

test("first run: the allowed login creates the first passkey", async ({ owner, world }) => {
  await owner.goto("/signin");
  await expect(owner.getByText("Create the passkey that guards changes to this hub")).toBeVisible();
  await expect(owner.getByText(/Open for \d+ more minutes/)).toBeVisible();
  await shot(owner, "first-run sign-in page");
  await enrolFirstRun(owner, world, "Owner laptop");
  await owner.goto("/settings");
  await expect(owner.locator("section", { has: owner.getByRole("heading", { name: "Passkeys" }) }).locator(".rows li")).toHaveCount(1);
  await shot(owner, "first passkey enrolled and signed in");
});

test("once a passkey exists, another device can't add one without a session", async ({ browser, world }, info) => {
  const { context, page, auth } = await otherDevice(browser, info, world);
  await page.goto("/signin");
  await expect(page.getByRole("button", { name: /Sign in with a passkey/ })).toBeVisible();
  await expect(page.getByRole("button", { name: "Create passkey" })).toHaveCount(0);
  await shot(page, "second device only sees sign-in");
  const res = await enrolStart(world, { token: "", name: "intruder" });
  expect(res.status).toBe(403);
  expect(res.body).toContain("Sign in to add another passkey");
  // Its own passkey isn't one the hub knows, so it can't sign in either.
  await page.getByRole("button", { name: /Sign in with a passkey/ }).click();
  await expect(page.locator("[data-error]")).toBeVisible();
  expect(await auth.credentials()).toEqual([]);
  await shot(page, "second device can't sign in");
  await context.close();
});

test("a signed-in owner adds a second passkey; its session's CSRF token is required", async ({ owner, world, auth }) => {
  const { cookie } = await ownerSecrets(owner);
  // A second authenticator beside the first, like plugging in a security key: the first
  // confirms the action (decision 0038: a fresh tap from an enrolled passkey) and, since it
  // already holds a credential for this hub, WebAuthn registers the new one on the second.
  await swapAuthenticator(owner, auth);
  const wrong = await enrolStart(world, { token: "", name: "x" }, { cookie: `__Host-atlas=${cookie.value}`, "x-atlas-csrf": "0".repeat(64) });
  expect(wrong.status).toBe(403);
  await owner.goto("/settings");
  await owner.getByPlaceholder("Name the new passkey").fill("Owner phone");
  await owner.getByRole("button", { name: "Add a passkey" }).click();
  await owner.waitForURL(/\/settings/);
  await expect(owner.locator("section", { has: owner.getByRole("heading", { name: "Passkeys" }) }).locator(".rows li")).toHaveCount(2);
  await shot(owner, "second passkey added while signed in");
});

test("a setup link from the hub adds a passkey once", async ({ browser, world }, info) => {
  const link = /(http\S+\/setup\/([0-9a-f]{64}))/.exec(hubCli(world, "setup"));
  const [url, token] = [link[1], link[2]];
  const { context, page } = await otherDevice(browser, info, world);
  await page.goto(url);
  await expect(page.getByText("Add a passkey to this hub. This link works once.")).toBeVisible();
  await shot(page, "setup link page");
  await page.getByPlaceholder(/Laptop, phone/).fill("Recovery key");
  await page.getByRole("button", { name: "Create passkey" }).click();
  await page.waitForURL(/\/settings/);
  await expect(page.getByText("Recovery key")).toBeVisible();
  await shot(page, "passkey added from the setup link");
  await context.close();
  // Attacker: someone who later finds the link in the shell history.
  const again = await request(world, { path: `/setup/${token}` });
  expect(again.status).toBe(404);
  expect(again.body).toContain("expired or was already used");
  const start = await enrolStart(world, { token, name: "intruder" });
  expect(start.status).toBe(403);
  const made = await enrolStart(world, { token: "0".repeat(64), name: "intruder" });
  expect(made.status).toBe(403);
});
