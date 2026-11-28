// A hub with no public_url is read-only: no passkeys, no sign-in, no edit controls.

import { test, expect, shot } from "../../lib/owner.mjs";

test.use({ enrol: false, worldOptions: { readOnly: true } });

test("a read-only hub shows everything and offers no way to change it", async ({ owner }) => {
  for (const [p, label] of [
    ["/skills/private/deploy", "skill"],
    ["/instructions/global/AGENTS.md", "instructions"],
    ["/memory/user-tz", "memory"],
  ]) {
    const res = await owner.goto(p);
    expect(res.status(), p).toBe(200);
    await expect(owner.locator("article.prose")).not.toBeEmpty();
    await expect(owner.locator(".edit-button")).toHaveCount(0);
    await shot(owner, `read-only ${label}`);
  }
  // No sign-in control in the rail either.
  await expect(owner.locator(".rail-foot .foot-link")).toHaveCount(0);
  await expect(owner.locator(".rail-foot form")).toHaveCount(0);

  for (const p of ["/signin", "/settings", "/skills/private/deploy/edit", "/memory/user-tz/edit", "/instructions/global/AGENTS.md/edit"]) {
    const res = await owner.request.get(p);
    expect(res.status(), p).toBe(404);
  }
});
