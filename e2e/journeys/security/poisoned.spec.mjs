// Poisoned content: a skill and a memory note committed with script, handlers, a hostile
// form, injected instructions and secret-looking tokens. Attacker: whoever can commit to
// the knowledge repo (a compromised agent writing memory, a bad pull request). Rendering
// must be sanitised; the architecture also promises a scanner that flags it.

import { test, expect, shot } from "../../lib/owner.mjs";
import { seedHostile } from "../../lib/security.mjs";

test.use({ worldOptions: { seedWith: seedHostile } });

const pages = ["/skills/private/poisoned", "/memory/poisoned"];

async function expectInert(page) {
  const article = page.locator("main .prose, main article").first();
  await expect(article).toContainText("IGNORE ALL PREVIOUS INSTRUCTIONS");
  for (const sel of ["script", "iframe", "form", "[onerror]", "[onclick]", 'a[href^="javascript:"]', 'a[href*="evil.example"]:not([rel*="noopener"])'])
    await expect(article.locator(sel), sel).toHaveCount(0);
  await page.locator("main").getByText("click me").first().click().catch(() => {});
  expect(await page.evaluate(() => window.__pwned ?? null)).toBeNull();
}

test("poisoned markdown renders inert", async ({ owner, ownerSession }) => {
  const dialogs = [];
  owner.on("dialog", (d) => (dialogs.push(d.message()), d.dismiss()));
  for (const p of pages) {
    await owner.goto(p);
    await expectInert(owner);
    await shot(owner, `rendered ${p}`);
  }
  expect(dialogs).toEqual([]);
  // The sanitiser keeps an <img>; the CSP stops it calling out (img-src 'self' data:).
  // Those refusals are the only console errors, so they're taken off the fixture's list.
  const problems = ownerSession.problems.splice(0);
  expect(problems.length).toBeGreaterThan(0);
  for (const p of problems) expect(p).toMatch(/evil\.example\/beacon\.png.*Content Security Policy|img-src/);
});

test("the editor shows the poison as text, not markup", async ({ owner }) => {
  for (const p of pages) {
    await owner.goto(`${p}/edit`);
    const text = await owner.locator("#editor-text").inputValue();
    expect(text).toContain('<script>window.__pwned = 1; alert("script")</script>');
    expect(await owner.locator("main script").count()).toBe(0);
    expect(await owner.evaluate(() => window.__pwned ?? null)).toBeNull();
    await shot(owner, `editing ${p}`);
  }
});

test("the scanner flags script, injected instructions and secret-looking tokens", async ({ owner }) => {
  test.fail();
  test.info().annotations.push({
    type: "bug",
    description: "No content scanner: the poisoned skill and memory note show no flag on their pages, in their lists or in Needs a look (architecture: 'The scanner flags anything risky').",
  });
  await owner.goto("/");
  const needs = owner.locator("section", { has: owner.getByRole("heading", { name: "Needs a look" }) });
  await expect(needs).toContainText("poisoned", { timeout: 3000 });
});
