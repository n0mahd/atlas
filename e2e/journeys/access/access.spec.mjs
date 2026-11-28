// Access: the first passkey on a fresh hub, signing out and in, a cancelled prompt, a setup
// link from another device, managing passkeys, the settings switches and CLI tokens.
// The tests run in order on one hub and one browser, like an owner's first evening.

import { test, expect, shot, addAuthenticator, watchConsole, allowConsole } from "../../lib/owner.mjs";
import { hubCli } from "../../lib/world.mjs";
import { isPhone, swapAuthenticator } from "../../lib/catalog.mjs";

test.use({ enrol: false });
test.describe.configure({ mode: "serial" });

const PHONE_RAIL_BUG =
  "Below 720px the rail hides .rail-foot (atlas.css `.rail-toggle, .rail-foot, .brand-word { display: none; }`), so on a phone Settings and Sign out are unreachable and Sign in is reachable only through an edit pencil.";

const keyNames = (page) => page.locator("section.section", { has: page.locator("h2", { hasText: "Passkeys" }) }).locator("ul.rows .row-name");

/** Signs out through the rail's button. On a phone the rail hides it (a bug, reported by
 *  its own test), so the same form is submitted directly to keep the other journeys going. */
async function signOut(page) {
  const button = page.getByRole("button", { name: "Sign out", exact: true });
  if (await button.isVisible()) await button.click();
  else await page.locator('form[action="/auth/signout"]').evaluate((f) => f.requestSubmit());
  await page.waitForLoadState("load");
  await expect(page.locator('meta[name="atlas-csrf"]')).toHaveCount(0);
}

async function signInHere(page) {
  await page.getByRole("button", { name: /Sign in with a passkey/ }).click();
  await page.waitForURL((u) => !u.pathname.startsWith("/signin"));
}

let phoneCredentials = [];

test("first run: the owner creates the first passkey", async ({ owner, world }) => {
  await owner.goto("/");
  await owner.goto("/signin");
  await expect(owner.locator("header.masthead h1")).toHaveText("Welcome to Atlas");
  await expect(owner.locator(".signin-lede")).toHaveText(
    `Signed in to Tailscale as ${world.login}. Create the passkey that guards changes to this hub.`,
  );
  await expect(owner.locator(".signin .editor-note")).toContainText(/Open for (29|30) more minutes/);
  await shot(owner, "first run");

  await owner.getByPlaceholder(/Laptop, phone/).fill("Laptop");
  await owner.getByRole("button", { name: "Create passkey" }).click();
  await expect(owner).toHaveURL(/\/settings$/);
  await expect(keyNames(owner)).toHaveText(["Laptop"]);
  await expect(owner.locator("main")).toContainText("Add a second passkey");
  await shot(owner, "first passkey enrolled");

  // Once a passkey exists, the first-run screen is gone for good.
  await signOut(owner);
  await owner.goto("/signin");
  await expect(owner.locator("header.masthead h1")).toHaveText("Sign in");
  await expect(owner.getByRole("button", { name: "Create passkey" })).toHaveCount(0);
  await signInHere(owner);
});

test("the rail offers Settings and Sign out, then Sign in", async ({ owner }, testInfo) => {
  if (isPhone(testInfo)) {
    test.info().annotations.push({ type: "bug", description: PHONE_RAIL_BUG });
    test.fail();
  }
  await owner.goto("/skills");
  await expect(owner.getByRole("link", { name: "Settings and passkeys" })).toBeVisible();
  await shot(owner, "rail signed in");
  await owner.getByRole("button", { name: "Sign out", exact: true }).click();
  await expect(owner.getByRole("link", { name: "Sign in with a passkey to edit" })).toBeVisible();
  await shot(owner, "rail signed out");
  await owner.getByRole("link", { name: "Sign in with a passkey to edit" }).click();
  await signInHere(owner);
  await expect(owner.getByRole("link", { name: "Settings and passkeys" })).toBeVisible();
});

test("signed out, the owner reads everything but saves nothing", async ({ owner, world }) => {
  allowConsole(owner, /status of 403/);
  await owner.goto("/skills/private/deploy");
  await signOut(owner);
  await owner.goto("/skills/private/deploy");
  await expect(owner.locator("article.prose")).toContainText("Run the deploy script.");
  await shot(owner, "signed out skill page");

  // Every catalog page still reads.
  for (const p of ["/", "/skills", "/instructions/global/AGENTS.md", "/memory/user-tz", "/hooks", "/mcp"]) {
    const res = await owner.goto(p);
    expect(res.status(), p).toBe(200);
  }

  // Settings and the editor both send the owner to sign in first.
  await owner.goto("/settings");
  await expect(owner).toHaveURL(/\/signin\?next=\/settings$/);
  await owner.goto("/skills/private/deploy/edit");
  await expect(owner).toHaveURL(/\/signin\?next=\/skills\/private\/deploy\/edit$/);
  await expect(owner.getByLabel("File text")).toHaveCount(0);
  await shot(owner, "edit asks for sign in");

  // A save posted without a session is refused and changes nothing.
  const res = await owner.request.post("/skills/private/deploy/edit", {
    form: { csrf: "", base: "", text: "overwritten" },
    headers: { origin: world.url },
  });
  expect(res.status()).toBe(403);
  expect(await res.text()).toContain("Sign in again; your text wasn't saved.");

  // Signing in from here lands in the editor the owner asked for.
  await signInHere(owner);
  await expect(owner).toHaveURL(/\/skills\/private\/deploy\/edit$/);
  await expect(owner.getByLabel("File text")).toHaveValue(/Run the deploy script\./);
  await shot(owner, "signed in, back in the editor");
});

test("a cancelled passkey prompt says so, and trying again works", async ({ owner, auth }) => {
  await owner.goto("/");
  await signOut(owner);
  await owner.goto("/signin");
  await auth.refuse();
  await owner.getByRole("button", { name: /Sign in with a passkey/ }).click();
  const error = owner.getByRole("alert");
  await expect(error).toBeVisible();
  await expect(error).toHaveText("The passkey prompt was closed. Try again.");
  await expect(owner).toHaveURL(/\/signin/);
  await expect(owner.locator('meta[name="atlas-csrf"]')).toHaveCount(0);
  await shot(owner, "sign in cancelled");

  await auth.refuse(false);
  await signInHere(owner);
  await expect(owner.locator('meta[name="atlas-csrf"]')).toHaveCount(1);
});

test("a setup link adds a passkey from another device, once", async ({ owner, world, browser }, testInfo) => {
  const out = hubCli(world, "setup");
  const link = out.match(/https?:\/\/\S+\/setup\/\S+/)?.[0];
  expect(link, out).toBeTruthy();
  expect(link.startsWith(world.url)).toBe(true);

  // The owner opens it on their phone, which has its own authenticator.
  const phone = await browser.newContext({ ...testInfo.project.use, baseURL: world.url });
  const page = await phone.newPage();
  const problems = watchConsole(page);
  const phoneAuth = await addAuthenticator(page);
  try {
    await page.goto(link);
    await expect(page.locator(".signin-lede")).toHaveText("Add a passkey to this hub. This link works once.");
    await shot(page, "setup link");
    await page.getByPlaceholder(/Laptop, phone/).fill("Phone");
    await page.getByRole("button", { name: "Create passkey" }).click();
    await expect(page).toHaveURL(/\/settings$/);
    await expect(keyNames(page)).toHaveText(["Laptop", "Phone"]);
    await shot(page, "phone enrolled");
    phoneCredentials = await phoneAuth.credentials();
    expect(phoneCredentials).toHaveLength(1);

    // The link is spent.
    const again = await page.goto(link);
    expect(again.status()).toBe(404);
    await expect(page.locator("body")).toContainText("This setup link has expired or was already used.");
    expect(problems.filter((p) => !/status of 404/.test(p))).toEqual([]);
  } finally {
    await phone.close();
  }

  await owner.goto("/settings");
  await expect(keyNames(owner)).toHaveText(["Laptop", "Phone"]);
  await expect(owner.locator("main")).not.toContainText("Add a second passkey");
});

test("Settings adds a passkey and removes others, never the last", async ({ owner, auth, world, browser }, testInfo) => {
  await owner.goto("/settings");
  // A security key: a second authenticator on this computer.
  const key = await swapAuthenticator(owner, auth);
  try {
    await owner.getByPlaceholder("Name the new passkey").fill("Security key");
    await owner.getByRole("button", { name: "Add a passkey" }).click();
    await expect(keyNames(owner)).toHaveText(["Laptop", "Phone", "Security key"]);
    expect(await key.credentials()).toHaveLength(1);
    await shot(owner, "three passkeys");
  } finally {
    await key.restore();
  }

  await owner.getByRole("button", { name: "Remove Security key" }).click();
  await expect(owner.locator(".flash")).toHaveText("Passkey removed.");
  await expect(keyNames(owner)).toHaveText(["Laptop", "Phone"]);
  await owner.getByRole("button", { name: "Remove Phone" }).click();
  await expect(keyNames(owner)).toHaveText(["Laptop"]);
  // The last passkey has no remove button, and the page asks for a second one.
  await expect(owner.getByRole("button", { name: /^Remove / })).toHaveCount(0);
  await expect(owner.locator("main")).toContainText("Add a second passkey");
  await shot(owner, "back to one passkey");

  // The removed phone passkey no longer signs in.
  const phone = await browser.newContext({ ...testInfo.project.use, baseURL: world.url });
  const page = await phone.newPage();
  await addAuthenticator(page, phoneCredentials);
  try {
    await page.goto("/signin");
    await page.getByRole("button", { name: /Sign in with a passkey/ }).click();
    await expect(page.getByRole("alert")).toBeVisible();
    await expect(page).toHaveURL(/\/signin/);
    await shot(page, "removed passkey refused");
  } finally {
    await phone.close();
  }
});

test("the settings switches save and change what Save does", async ({ owner }) => {
  await owner.goto("/settings");
  const send = owner.getByRole("checkbox", { name: /Send changes to my machines when I save/ });
  const push = owner.getByRole("checkbox", { name: /Also push project repos after saving/ });
  await expect(send).toBeChecked();
  await expect(push).not.toBeChecked();

  await owner.locator("label.switch", { hasText: "Send changes to my machines" }).click();
  await owner.locator("label.switch", { hasText: "Also push project repos" }).click();
  await expect(send).not.toBeChecked();
  await expect(push).toBeChecked();
  await owner.getByRole("button", { name: "Save", exact: true }).click();
  await expect(owner.locator(".flash")).toHaveText("Settings saved.");
  await owner.reload();
  await expect(send).not.toBeChecked();
  await expect(push).toBeChecked();
  await shot(owner, "settings saved");

  // The editor says what Save will now do.
  await owner.goto("/skills/shop/stock/edit");
  await expect(owner.locator(".editor-note")).toHaveText("Saving commits to shop's main branch and pushes it, which may deploy it.");
  await owner.goto("/skills/private/deploy/edit");
  await expect(owner.locator(".editor-note")).toHaveText("Saving commits to the knowledge repo.");
  await expect(owner.locator("form.editor")).not.toHaveAttribute("data-send-on-save", /.*/);
  await shot(owner, "editor without sending");

  await owner.goto("/settings");
  await owner.locator("label.switch", { hasText: "Send changes to my machines" }).click();
  await owner.locator("label.switch", { hasText: "Also push project repos" }).click();
  await owner.getByRole("button", { name: "Save", exact: true }).click();
  await expect(owner.locator(".flash")).toHaveText("Settings saved.");
  await expect(send).toBeChecked();
  await expect(push).not.toBeChecked();
});

test("a CLI token is shown once, works, and stops working when revoked", async ({ owner, world }) => {
  await owner.goto("/settings");
  const name = owner.getByLabel("Name the CLI token");

  // A name of only spaces is refused with a reason.
  await name.fill("   ");
  await owner.getByRole("button", { name: "Create token" }).click();
  await expect(owner.locator("#cli-tokens [role=alert]:not([hidden])")).toHaveText("Name the token first, such as laptop.");
  await shot(owner, "token needs a name");

  await owner.getByLabel("Name the CLI token").fill("workstation");
  await owner.getByRole("button", { name: "Create token" }).click();
  const tokens = owner.locator("#cli-tokens");
  await expect(tokens.getByRole("status")).toContainText("Token for workstation (shown once)");
  const command = await tokens.locator("pre.ink-block").innerText();
  const raw = command.match(new RegExp(`^atlasd auth ${world.url} (\\S+)$`))?.[1];
  expect(raw, command).toBeTruthy();
  await expect(tokens.getByRole("button", { name: "Copy" })).toBeVisible();
  await expect(tokens.locator("ul.rows .row-name")).toHaveText(["workstation"]);
  await expect(tokens.locator("ul.rows .row-desc")).toContainText("last used never");
  await shot(owner, "token created");

  // The token works for the workstation CLI's API.
  const api = (t) => owner.request.get("/api/admin/machines", { headers: { authorization: `Bearer ${t}` } });
  expect((await api(raw)).status()).toBe(200);

  // Shown once: a reload lists the token but never its secret.
  await owner.goto("/settings");
  await expect(owner.locator("#cli-tokens pre")).toHaveCount(0);
  expect(await owner.content()).not.toContain(raw);
  await expect(owner.locator("#cli-tokens ul.rows .row-name")).toHaveText(["workstation"]);
  await shot(owner, "token listed, secret gone");

  await owner.getByRole("button", { name: "Revoke workstation" }).click();
  await expect(owner.locator(".flash")).toHaveText("CLI token revoked.");
  await expect(owner.locator("#cli-tokens ul.rows .row-name")).toHaveCount(0);
  await shot(owner, "token revoked");
  expect((await api(raw)).status()).toBe(401);
});

test("Sign out everywhere ends every session, this one too", async ({ owner, world, auth, browser }) => {
  // A second browser (the owner's phone, say) signed in with the same passkey.
  // The passkey is synced to it: one credential, one signature counter.
  const other = await browser.newContext({ baseURL: world.url });
  const phone = await other.newPage();
  const phoneAuth = await addAuthenticator(phone, await auth.credentials());
  await phone.goto("/signin?next=%2Fsettings");
  await phone.getByRole("button", { name: /Sign in with a passkey/ }).click();
  await phone.waitForURL(/\/settings/);
  await expect(phone.locator('meta[name="atlas-csrf"]')).toHaveCount(1);
  // Sync the counter back, or the hub rightly refuses the laptop's older count as a clone.
  for (const credential of await phoneAuth.credentials()) {
    await auth.cdp.send("WebAuthn.removeCredential", { authenticatorId: auth.authenticatorId, credentialId: credential.credentialId });
    await auth.cdp.send("WebAuthn.addCredential", { authenticatorId: auth.authenticatorId, credential });
  }

  await owner.goto("/settings");
  await shot(owner, "before signing out everywhere");
  await owner.getByRole("button", { name: "Sign out all" }).click();
  await owner.goto("/settings");
  await expect(owner).toHaveURL(/\/signin/);
  await shot(owner, "this browser signed out");
  await phone.reload();
  await expect(phone).toHaveURL(/\/signin/);
  await expect(phone.locator('meta[name="atlas-csrf"]')).toHaveCount(0);
  await other.close();

  // The passkey still works: sign back in.
  await owner.getByRole("button", { name: /Sign in with a passkey/ }).click();
  await owner.waitForURL((u) => !u.pathname.startsWith("/signin"));
  await expect(owner.locator('meta[name="atlas-csrf"]')).toHaveCount(1);
});
