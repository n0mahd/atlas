// Headers: every response carries the hub's strict CSP and the other headers
// security_headers() in crates/atlas-web/src/lib.rs promises, and no served HTML holds
// inline script, inline handlers or inline style that the CSP would have to allow.

import http from "node:http";
import { test, expect, shot } from "../../lib/owner.mjs";
import { ownerSecrets, request } from "../../lib/security.mjs";

test.use({ worldOptions: { label: "headers" } });

const CSP =
  "default-src 'none'; script-src 'self'; style-src 'self'; font-src 'self'; img-src 'self' data:; " +
  "connect-src 'self'; manifest-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'";

const pages = [
  "/", "/skills", "/skills/private/deploy", "/skills/public/review", "/skills/shop/stock",
  "/instructions", "/instructions/global/AGENTS.md", "/instructions/shop/AGENTS.md",
  "/memory", "/memory/user-tz", "/hooks", "/mcp", "/machines", "/settings", "/signin",
  "/skills/private/deploy/edit", "/memory/user-tz/edit", "/instructions/global/AGENTS.md/edit",
];
// Everything else a GET can reach, including redirects and errors.
const others = ["/releases", "/install", "/install/atlasd", "/setup/0000", "/no-such-page", "/static/atlas.css", "/static/atlas.js", "/static/fonts/cinzel.woff2"];

function expectHeaders(res, p) {
  const h = res.headers;
  expect(h["content-security-policy"], p).toBe(CSP);
  expect(h["x-content-type-options"], p).toBe("nosniff");
  expect(h["x-frame-options"], p).toBe("DENY");
  expect(h["referrer-policy"], p).toBe("same-origin");
  expect(h["cross-origin-opener-policy"], p).toBe("same-origin");
  expect(h["permissions-policy"], p).toBe("camera=(), microphone=(), geolocation=(), interest-cohort=()");
  if (!p.startsWith("/static/")) expect(h["cache-control"], p).toBe("no-store");
}

function expectNoInline(html, p) {
  const tags = html.match(/<[a-zA-Z][^>]*>/g) ?? [];
  expect(tags.filter((t) => /^<script\b/i.test(t) && !/\bsrc=/i.test(t)), `${p}: inline <script>`).toEqual([]);
  expect(tags.filter((t) => /\son[a-z]+\s*=/i.test(t)), `${p}: on*= handlers`).toEqual([]);
  expect(tags.filter((t) => /\sstyle\s*=/i.test(t)), `${p}: style= attributes`).toEqual([]);
  expect(html, `${p}: <style> blocks`).not.toMatch(/<style\b/i);
  expect(html, `${p}: javascript: URLs`).not.toMatch(/(href|src|action)\s*=\s*["']?\s*javascript:/i);
  for (const src of html.match(/<script[^>]*\bsrc="([^"]+)"/g) ?? []) expect(src, p).toMatch(/src="\/static\//);
}

test("every page, signed in and signed out, carries the headers and no inline script", async ({ owner, world }) => {
  const { cookie } = await ownerSecrets(owner);
  for (const session of [{ cookie: `__Host-atlas=${cookie.value}` }, {}]) {
    for (const p of [...pages, ...others]) {
      const res = await request(world, { path: p, headers: session });
      expectHeaders(res, p);
      if ((res.headers["content-type"] ?? "").startsWith("text/html")) expectNoInline(res.body, p);
    }
  }
});

test("pages render in the browser with no CSP violation", async ({ owner }) => {
  // The owner fixture fails the test on any console error, which includes CSP reports.
  for (const p of pages) {
    const res = await owner.goto(p);
    expect(res.status(), p).toBe(200);
  }
  await owner.goto("/");
  const htmx = JSON.parse(await owner.locator('meta[name="htmx-config"]').getAttribute("content"));
  expect(htmx).toMatchObject({ allowEval: false, allowScriptTags: false, selfRequestsOnly: true });
  await shot(owner, "front page under the strict CSP");
});

test("another site can't frame the hub", async ({ owner, world }) => {
  // Attacker: a clickjacking page on another site, framing a signed-in page.
  const html = `<!doctype html><title>Frame</title><p>outside</p><iframe src="${world.url}/settings" width="600" height="400"></iframe>`;
  const site = http.createServer((_, res) => res.writeHead(200, { "content-type": "text/html" }).end(html));
  await new Promise((r) => site.listen(0, "127.0.0.1", r));
  const page = await owner.context().newPage();
  await page.goto(`http://127.0.0.1:${site.address().port}/`);
  await page.waitForTimeout(500);
  const frame = page.frames().find((f) => f !== page.mainFrame());
  const framed = await frame.evaluate(() => document.body?.innerText ?? "").catch(() => "");
  expect(framed).not.toMatch(/passkeys|cli tokens/i);
  test.info().annotations.push({ type: "frame url", description: frame.url() });
  expect(frame.url()).not.toBe(`${world.url}/settings`);
  await shot(page, "hub refused inside a frame");
  await page.close();
  site.close();
});
