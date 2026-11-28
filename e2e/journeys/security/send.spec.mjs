// Sending an update needs a passkey session, its CSRF token, a same-origin request, and
// a fresh passkey assertion over this update. Attackers: someone holding a stolen session
// cookie and CSRF token but not the passkey; and the owner closing the passkey prompt.
// Whether anything went out is checked the way machines see it (GET /api/release via
// atlasd) and in Settings → Updates.

import crypto from "node:crypto";
import { test, expect, shot, manualReview } from "../../lib/owner.mjs";
import {
  atlasd,
  ownerSecrets,
  pairCodeFromUi,
  pairedMachine,
  request,
  sendNow,
  servedRelease,
} from "../../lib/security.mjs";

test.use({ worldOptions: { label: "send" } });
test.describe.configure({ mode: "serial" });

let laptop;
let first; // update 1 as served, with the owner's genuine assertion

const json = (world, route, body, { cookie, csrf, origin = world.url } = {}) =>
  request(world, {
    method: "POST",
    path: route,
    headers: {
      "content-type": "application/json",
      ...(origin ? { origin } : {}),
      ...(cookie ? { cookie: `__Host-atlas=${cookie}` } : {}),
      ...(csrf ? { "x-atlas-csrf": csrf } : {}),
    },
    body: JSON.stringify(body),
  });

async function updatesShown(owner) {
  await owner.goto("/settings#updates");
  return owner.locator("#updates .rows li").count();
}

test("closing the passkey prompt sends nothing", async ({ owner, world, auth }) => {
  laptop = await pairedMachine(world, "laptop", await pairCodeFromUi(owner));
  await owner.goto("/settings#updates");
  await auth.refuse();
  manualReview(owner);
  try {
    await owner.getByRole("button", { name: "Send now" }).click();
    // The first update lists every hook, so the tap is on "Check before sending" (0040).
    await owner.waitForURL(/\/send\/review\//);
    await owner.locator("section[data-pending] [data-action='send']").click();
    await expect(owner.locator("section[data-pending] [data-error]")).toBeVisible();
    await shot(owner, "passkey prompt closed, nothing sent");
  } finally {
    manualReview(owner, false);
    await auth.refuse(false);
  }
  expect(await updatesShown(owner)).toBe(0);
  expect(await servedRelease(world, laptop)).toBeNull();
});

test("send needs the session, its CSRF token and the hub's origin", async ({ owner, world }) => {
  const { cookie, csrf } = await ownerSecrets(owner);
  const other = crypto.randomBytes(32).toString("hex");
  for (const route of ["/send/start", "/send/finish"]) {
    const body = route === "/send/start" ? { next: "/" } : { credential: { credential_id: "x", authenticator_data: "x", client_data_json: "x", signature: "x" } };
    for (const as of [{ csrf }, { cookie: cookie.value }, { cookie: cookie.value, csrf: other }, { cookie: cookie.value, csrf, origin: "http://evil.example" }, { cookie: cookie.value, csrf, origin: null }]) {
      const res = await json(world, route, body, as);
      expect(res.status, `${route} ${JSON.stringify({ ...as, cookie: !!as.cookie })}`).toBe(403);
    }
  }
  expect(await servedRelease(world, laptop)).toBeNull();
});

/** A WebAuthn assertion over the hub's `challenge` (base64url), made with `privateKey`. */
function assertion(world, challenge, privateKey, credentialId) {
  const authData = Buffer.concat([crypto.createHash("sha256").update("localhost").digest(), Buffer.from([0x05, 0, 0, 0, 9])]);
  const clientData = Buffer.from(JSON.stringify({ type: "webauthn.get", challenge, origin: world.url, crossOrigin: false }));
  const signed = Buffer.concat([authData, crypto.createHash("sha256").update(clientData).digest()]);
  return {
    credential_id: credentialId,
    authenticator_data: authData.toString("base64url"),
    client_data_json: clientData.toString("base64url"),
    signature: crypto.sign("sha256", signed, privateKey).toString("base64url"),
  };
}

test("a stolen session without the passkey can't sign an update", async ({ owner, world, auth }) => {
  // Attacker: has the cookie and CSRF token, can start a send, but holds only its own key.
  const { cookie, csrf } = await ownerSecrets(owner);
  const as = { cookie: cookie.value, csrf };
  const [cred] = await auth.credentials();
  const ownerId = Buffer.from(cred.credentialId, "base64").toString("base64url");
  const attacker = crypto.generateKeyPairSync("ec", { namedCurve: "P-256" });
  for (const credentialId of [crypto.randomBytes(16).toString("base64url"), ownerId]) {
    // The first update lists every hook, so it goes through "Check before sending" (0040);
    // the stolen session can load that page too, so the attacker does, and then signs.
    let start = await json(world, "/send/start", { next: "/settings" }, as);
    expect(start.status, start.body).toBe(200);
    const { review } = JSON.parse(start.body);
    let pending = "";
    if (review) {
      pending = review.split("/").pop();
      const page = await request(world, { path: review, headers: { cookie: `__Host-atlas=${as.cookie}` } });
      expect(page.status).toBe(200);
      start = await json(world, "/send/start", { next: "/settings", pending }, as);
      expect(start.status, start.body).toBe(200);
    }
    const { challenge, pending: id } = JSON.parse(start.body);
    const finish = await json(world, "/send/finish", { credential: assertion(world, challenge, attacker.privateKey, credentialId), next: "/settings", pending: id ?? pending }, as);
    expect(finish.status, finish.body).toBe(403);
    expect(finish.body).toContain("That signature didn't check out, so nothing was sent.");
  }
  expect(await updatesShown(owner)).toBe(0);
  expect(await servedRelease(world, laptop)).toBeNull();
  await shot(owner, "no update after the forged signatures");
});

test("the owner's passkey sends; its assertion can't be reused for another update", async ({ owner, world, auth }) => {
  expect(await sendNow(owner)).toBe(1);
  await shot(owner, "update 1 sent with the passkey");
  first = await servedRelease(world, laptop);
  expect(first.number).toBe(1);

  // A new change, saved but not sent: the owner closes the passkey prompt.
  await owner.goto("/memory/user-tz/edit");
  await owner.locator("#editor-text").fill("---\nname: user-tz\ndescription: The owner's time zone\nmetadata:\n  type: user\n  host: all\n---\nPacific.\n");
  await auth.refuse();
  await owner.getByRole("button", { name: "Save" }).click();
  await owner.waitForURL(/notsent=1/);
  await auth.refuse(false);
  await expect(owner.getByRole("status")).toContainText("Saved. Not on your machines yet");
  await shot(owner, "saved, not sent");

  // Attacker with the session: starts update 2 and finishes it with update 1's genuine
  // owner assertion, which was made for a different challenge.
  const { cookie, csrf } = await ownerSecrets(owner);
  const start = await json(world, "/send/start", { next: "/" }, { cookie: cookie.value, csrf });
  expect(start.status, start.body).toBe(200);
  const finish = await json(world, "/send/finish", { credential: first.signature, next: "/" }, { cookie: cookie.value, csrf });
  expect(finish.status).toBe(403);
  expect((await servedRelease(world, laptop)).number).toBe(1);
  expect(await updatesShown(owner)).toBe(1);

  // The owner's own tap sends it.
  expect(await sendNow(owner)).toBe(2);
  expect((await servedRelease(world, laptop)).number).toBe(2);
  const run = await atlasd(laptop, "run");
  expect(run.out).toContain("Update 2 is applied.");
});
