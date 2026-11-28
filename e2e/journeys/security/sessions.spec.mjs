// Sessions and CSRF: every state-changing route needs the owner's passkey session, that
// session's CSRF token and a same-origin request. The attackers: another tailnet device
// of the owner's with no session (raw HTTP through the proxy), a page on another site in
// the owner's own browser (a real cross-site form), and someone replaying an old cookie.

import crypto from "node:crypto";
import http from "node:http";
import { test, expect, shot, signIn } from "../../lib/owner.mjs";
import { git } from "../../lib/world.mjs";
import { form, ownerSecrets, pairCodeFromUi, pairedMachine, request } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "sessions" } });
test.describe.configure({ mode: "serial" });

const EVIL = "http://evil.example";
let ids; // real ids, so a missing check would really do damage

/**
 * Everything a write could change, as the owner sees it: each repo's HEAD, and what
 * Settings and Machines show (passkeys, CLI tokens, switches, updates, machines, clients).
 */
async function fingerprint(world, owner) {
  const texts = async (sel) => (await owner.locator(sel).allInnerTexts()).map((t) => t.replace(/last used .*|seen .*/g, ""));
  await owner.goto("/settings");
  const settings = {
    updates: await texts("#updates .rows li"),
    keys: await texts("section:has(h2:text('Passkeys')) .rows li"),
    tokens: await texts("#cli-tokens .rows li"),
    switches: await owner.locator('#updates input[type="checkbox"]').evaluateAll((els) => els.map((e) => `${e.name}=${e.checked}`)),
  };
  await owner.goto("/machines");
  const machines = {
    names: await texts("section.ledger h2"),
    clients: await owner.locator('form[action^="/machines/"][action$="/remove"]').evaluateAll((els) => els.map((e) => e.getAttribute("action"))),
  };
  return {
    settings,
    machines,
    knowledge: git(world, world.knowledge, "rev-parse", "HEAD"),
    skills: git(world, world.publicSkills, "rev-parse", "HEAD"),
    shop: git(world, `${world.projectsDir}/shop`, "rev-parse", "HEAD"),
  };
}

/** Every POST route in crates/atlas-web/src/lib.rs that needs a session. */
function writes() {
  const cred = { credential_id: "x", authenticator_data: "x", client_data_json: "x", signature: "x" };
  return [
    ["/auth/enrol/start", "json", { token: "", name: "evil" }],
    ["/auth/signout", "form", {}],
    ["/settings", "form", { send_on_save: "1", push_after_save: "1" }],
    [`/settings/keys/${ids.key}/remove`, "form", {}],
    ["/settings/tokens", "form", { name: "evil" }],
    [`/settings/tokens/${ids.token}/remove`, "form", {}],
    ["/send/start", "json", { next: "/" }],
    ["/send/finish", "json", { credential: cred, next: "/" }],
    ["/machines/pair", "form", { machine: "" }],
    ["/machines/pair/ssh", "form", { target: "evil@example.invalid" }],
    ["/machines/add", "form", { name: "evil" }],
    ["/machines/forget", "form", { name: "laptop" }],
    [`/machines/${ids.client}/remove`, "form", {}],
    ["/skills/private/deploy/edit", "form", { base: ids.bases.skill, text: "# pwned\n" }],
    ["/memory/user-tz/edit", "form", { base: ids.bases.memory, text: "pwned\n" }],
    ["/instructions/global/AGENTS.md/edit", "form", { base: ids.bases.instructions, text: "pwned\n" }],
  ];
}

/** A POST as the attacker shapes it: which cookie, CSRF token and Origin it carries. */
function post(world, route, kind, fields, { cookie, csrf, origin, site }) {
  const headers = {};
  if (cookie) headers.cookie = `__Host-atlas=${cookie}`;
  if (origin) headers.origin = origin;
  if (site) headers["sec-fetch-site"] = site;
  let body;
  if (kind === "json") {
    headers["content-type"] = "application/json";
    if (csrf) headers["x-atlas-csrf"] = csrf;
    body = JSON.stringify(fields);
  } else {
    headers["content-type"] = "application/x-www-form-urlencoded";
    body = form(csrf ? { csrf, ...fields } : fields);
  }
  return request(world, { method: "POST", path: route, headers, body });
}

test("cookies are HttpOnly, Secure, SameSite=Strict and host-only", async ({ owner }) => {
  const { cookie } = await ownerSecrets(owner);
  expect(cookie).toMatchObject({ name: "__Host-atlas", httpOnly: true, secure: true, sameSite: "Strict", path: "/" });
  expect(cookie.value).toMatch(/^[0-9a-f]{64}$/);
  // localhost counts as a secure context, so Chromium keeps a Secure cookie on plain http.
  expect(await owner.evaluate(() => document.cookie)).not.toContain("__Host-atlas");
});

test("setup: real ids for the passkey, a CLI token, a paired client and each file", async ({ owner, world, auth }) => {
  const [cred] = await auth.credentials();
  await owner.goto("/settings#cli-tokens");
  await owner.getByPlaceholder("Name this workstation").fill("workstation");
  await owner.getByRole("button", { name: "Create token" }).click();
  await expect(owner.getByText("(shown once)")).toBeVisible();
  const token = (await owner.locator('form[action^="/settings/tokens/"]').getAttribute("action")).split("/")[3];
  await pairedMachine(world, "laptop", await pairCodeFromUi(owner));
  await owner.goto("/machines");
  const client = (await owner.locator('form[action$="/remove"][action^="/machines/"]').first().getAttribute("action")).split("/")[2];
  const bases = {};
  for (const [k, p] of [["skill", "/skills/private/deploy/edit"], ["memory", "/memory/user-tz/edit"], ["instructions", "/instructions/global/AGENTS.md/edit"]]) {
    await owner.goto(p);
    bases[k] = await owner.locator('input[name="base"]').getAttribute("value");
  }
  ids = { key: Buffer.from(cred.credentialId, "base64").toString("base64url"), token, client, bases };
});

test("every write is refused without a session, with a wrong CSRF token, or from another origin", async ({ owner, world }) => {
  const { cookie, csrf } = await ownerSecrets(owner);
  // Control: the same request shape with everything right is accepted.
  const ok = await post(world, "/settings", "form", { send_on_save: "1" }, { cookie: cookie.value, csrf, origin: world.url });
  expect(ok.status).toBe(303);

  const before = await fingerprint(world, owner);
  const other = crypto.randomBytes(32).toString("hex");
  const attempts = {
    "no session": { csrf, origin: world.url },
    "an unknown session": { cookie: other, csrf, origin: world.url },
    "a wrong CSRF token": { cookie: cookie.value, csrf: other, origin: world.url },
    "no CSRF token": { cookie: cookie.value, origin: world.url },
    "a foreign Origin": { cookie: cookie.value, csrf, origin: EVIL },
    "a null Origin": { cookie: cookie.value, csrf, origin: "null" },
    "no Origin, cross-site": { cookie: cookie.value, csrf, site: "cross-site" },
    "no Origin, no fetch metadata": { cookie: cookie.value, csrf },
  };
  const refused = [];
  for (const [route, kind, fields] of writes()) {
    for (const [what, as] of Object.entries(attempts)) {
      const res = await post(world, route, kind, fields, as);
      expect(res.status, `${route} with ${what}: ${res.body}`).toBe(403);
      refused.push(`${route} · ${what}`);
    }
  }
  test.info().annotations.push({ type: "refused", description: `${refused.length} attempts on ${writes().length} routes` });
  expect(await fingerprint(world, owner), "no write landed").toEqual(before);
  // The owner's session still works and nothing changed on screen.
  await owner.goto("/settings");
  await expect(owner.locator("#cli-tokens .rows li")).toHaveCount(1);
  await expect(owner.getByText("evil")).toHaveCount(0);
  await shot(owner, "settings unchanged after the attempts");
  await owner.goto("/machines");
  await expect(owner.getByRole("heading", { name: "evil" })).toHaveCount(0);
  await expect(owner.getByRole("heading", { name: "laptop" })).toBeVisible();
  await shot(owner, "machines unchanged after the attempts");
});

test("passkey ceremonies refuse another origin", async ({ world }) => {
  const reg = { id: "x", rawId: "eA", type: "public-key", extensions: {}, response: { attestationObject: "eA", clientDataJSON: "eA" } };
  const get = { id: "x", rawId: "eA", type: "public-key", extensions: {}, response: { authenticatorData: "eA", clientDataJSON: "eA", signature: "eA", userHandle: null } };
  const ceremonies = [
    ["/auth/enrol/start", { token: "", name: "evil" }],
    ["/auth/enrol/finish", { flow: "x", credential: reg }],
    ["/auth/signin/start", { next: "/" }],
    ["/auth/signin/finish", { flow: "x", credential: get }],
  ];
  for (const [route, body] of ceremonies) {
    for (const as of [{ origin: EVIL }, { site: "cross-site" }, {}]) {
      const res = await post(world, route, "json", body, as);
      expect(res.status, `${route} ${JSON.stringify(as)}: ${res.body}`).toBe(403);
    }
  }
  // A same-origin finish with a made-up ceremony id is refused too.
  const res = await post(world, "/auth/signin/finish", "json", { flow: "x", credential: get }, { origin: world.url });
  expect(res.status).toBe(403);
});

test("a cross-site form in the owner's own browser can't write", async ({ owner, world }) => {
  // Attacker: a page on another site the owner visits, which even knows the CSRF token.
  const { csrf } = await ownerSecrets(owner);
  const html = `<!doctype html><title>Win a prize</title>
<form id="f" method="post" action="${world.url}/settings/tokens"><input name="csrf" value="${csrf}"><input name="name" value="evil-site"></form>
<script>document.getElementById("f").submit()</script>`;
  const site = http.createServer((_, res) => res.writeHead(200, { "content-type": "text/html" }).end(html));
  await new Promise((r) => site.listen(0, "127.0.0.1", r));
  const page = await owner.context().newPage();
  const answer = page.waitForResponse((r) => r.url().startsWith(`${world.url}/settings/tokens`));
  await page.goto(`http://127.0.0.1:${site.address().port}/`);
  const res = await answer;
  expect(res.status()).toBe(403);
  await page.waitForLoadState();
  await shot(page, "cross-site form refused");
  await page.close();
  site.close();
  await owner.goto("/settings#cli-tokens");
  await expect(owner.getByText("evil-site")).toHaveCount(0);
});

test("the owner can find Sign out on every width", async ({ owner }, info) => {
  if (info.project.name === "phone") {
    test.fail();
    info.annotations.push({
      type: "bug",
      description: "No Sign out control at phone width: the rail footer (Sign out, Settings, Sign in to edit) is hidden below 720px.",
    });
  }
  await owner.goto("/skills");
  await expect(owner.getByRole("button", { name: "Sign out", exact: true })).toBeVisible({ timeout: 3000 });
});

test("signing out ends the session: the old cookie replayed is refused", async ({ owner, world }, info) => {
  const { cookie, csrf } = await ownerSecrets(owner);
  await owner.goto("/settings");
  const out = owner.waitForResponse((r) => r.url().endsWith("/auth/signout"));
  if (info.project.name === "phone") {
    // The button is hidden at this width (see the test above): submit the same form it would.
    await owner.locator('form[action="/auth/signout"]').evaluate((f) => f.submit());
  } else {
    await owner.getByRole("button", { name: "Sign out", exact: true }).click();
  }
  const cleared = await (await out).headerValue("set-cookie");
  expect(cleared).toMatch(/__Host-atlas=;.*Max-Age=0/);
  await expect(owner.locator('meta[name="atlas-csrf"]')).toHaveCount(0);
  await shot(owner, "signed out");

  // Attacker: someone who copied the cookie and token before the owner signed out.
  const replay = await post(world, "/settings/tokens", "form", { name: "replayed" }, { cookie: cookie.value, csrf, origin: world.url });
  expect(replay.status).toBe(403);
  const page = await request(world, { path: "/settings", headers: { cookie: `__Host-atlas=${cookie.value}` } });
  expect(page.status).toBe(303);
  expect(page.headers.location).toBe("/signin?next=/settings");

  // Signing in again starts a new session with a new cookie, set with every flag.
  const finish = owner.waitForResponse((r) => r.url().endsWith("/auth/signin/finish"));
  await signIn(owner, world, "/settings");
  const set = await (await finish).headerValue("set-cookie");
  for (const flag of ["Path=/", "Secure", "HttpOnly", "SameSite=Strict"]) expect(set).toContain(flag);
  expect(set).not.toMatch(/Domain=/i);
  const fresh = await ownerSecrets(owner);
  expect(fresh.cookie.value).not.toBe(cookie.value);
  expect(fresh.csrf).not.toBe(csrf);
  await shot(owner, "signed in again with a new session");
});
