// Visual regression: every screen at desktop and phone width, in the day and night
// editions, compared with the committed baselines in e2e/baselines/. Text that changes per
// run (dates, ports, one-time codes) is masked. Update with --update-snapshots after an
// intended design change, then look at every changed image before committing it.
// No `stylePath`: Playwright injects it as an inline <style>, which the hub's CSP blocks
// (and the owner fixture fails on the violation). `animations: "disabled"` in the config
// already stops transitions.

import { test, expect } from "../../lib/owner.mjs";
import { createWorld, destroyWorld, hubCli } from "../../lib/world.mjs";
import { SCHEMES, SCREENS, masks, settle, setupLinkFrom, strangerPage } from "../../lib/screens.mjs";

// Its own world, so journeys in other files can't change what these screens show.
test.use({ worldOptions: { purpose: "visual" } });

const snap = (page, name, mask = []) =>
  expect(page).toHaveScreenshot(name, { fullPage: true, mask });

for (const scheme of SCHEMES) {
  for (const screen of SCREENS) {
    test(`${screen.name} (${scheme})`, async ({ owner }) => {
      await owner.emulateMedia({ colorScheme: scheme });
      const res = await owner.goto(screen.path);
      expect(res.status()).toBe(200);
      await settle(owner);
      await snap(owner, `${screen.name}-${scheme}.png`, masks(owner, screen));
    });
  }

  test(`skills with a drawer open (${scheme})`, async ({ owner }) => {
    await owner.emulateMedia({ colorScheme: scheme });
    await owner.goto("/skills");
    await owner.locator("summary", { hasText: "deploy" }).click();
    await expect(owner.getByRole("link", { name: "Read the skill" }).first()).toBeVisible();
    await settle(owner);
    await snap(owner, `skills-open-${scheme}.png`);
  });

  test(`machines with a pairing command (${scheme})`, async ({ owner }) => {
    await owner.emulateMedia({ colorScheme: scheme });
    await owner.goto("/machines");
    await owner.getByRole("button", { name: "Pair", exact: true }).first().click();
    await expect(owner.getByRole("heading", { name: "Pair a machine" })).toBeVisible();
    await settle(owner);
    // The command holds the hub's port and a one-time code.
    await snap(owner, `machines-pair-${scheme}.png`, [owner.locator(".code-row pre")]);
  });

  test(`rail collapsed (${scheme})`, async ({ owner }, info) => {
    test.skip(info.project.name === "phone", "phones get a top bar; the rail toggle is desktop only");
    await owner.emulateMedia({ colorScheme: scheme });
    await owner.goto("/skills");
    const toggle = owner.getByRole("button", { name: "Collapse or expand the menu" });
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await settle(owner);
    await snap(owner, `rail-collapsed-skills-${scheme}.png`);
    // Remembered per browser: the next page opens collapsed too.
    await owner.goto("/memory");
    await expect(owner.locator("html")).toHaveAttribute("data-rail", "collapsed");
    await settle(owner);
    await snap(owner, `rail-collapsed-memory-${scheme}.png`);
    await owner.getByRole("button", { name: "Collapse or expand the menu" }).click();
    await expect(owner.locator("html")).not.toHaveAttribute("data-rail", "collapsed");
  });

  // `owner` enrols the world's passkey, so the stranger gets the sign-in screen.
  test(`sign-in: passkey (${scheme})`, async ({ owner, browser, world }, info) => {
    const s = await strangerPage(browser, world, info);
    await s.page.emulateMedia({ colorScheme: scheme });
    await s.page.goto("/signin");
    await expect(s.page.getByRole("button", { name: /Sign in with a passkey/ })).toBeVisible();
    await settle(s.page);
    await snap(s.page, `signin-passkey-${scheme}.png`);
    expect(s.problems).toEqual([]);
    await s.context.close();
  });

  test(`sign-in: setup link (${scheme})`, async ({ browser, world }, info) => {
    const link = setupLinkFrom(hubCli(world, "setup"));
    const s = await strangerPage(browser, world, info);
    await s.page.emulateMedia({ colorScheme: scheme });
    await s.page.goto(link);
    await expect(s.page.getByRole("button", { name: "Create passkey" })).toBeVisible();
    await settle(s.page);
    await snap(s.page, `signin-setup-link-${scheme}.png`);
    expect(s.problems).toEqual([]);
    await s.context.close();
  });

  for (const [mode, opts, handle] of [
    ["first run", { firstRunMinutes: 30 }, "Create passkey"],
    ["closed", { firstRunMinutes: 0 }, null],
  ]) {
    test(`sign-in: ${mode} (${scheme})`, async ({ browser }, info) => {
      const fresh = await createWorld(`${info.project.name}-signin-${mode.replace(" ", "-")}`, opts);
      try {
        const s = await strangerPage(browser, fresh, info);
        await s.page.emulateMedia({ colorScheme: scheme });
        await s.page.goto("/signin");
        if (handle) await expect(s.page.getByRole("button", { name: handle })).toBeVisible();
        else await expect(s.page.getByText("atlas-hub setup", { exact: true })).toBeVisible();
        await settle(s.page);
        // The first-run note counts down minutes.
        await snap(s.page, `signin-${mode.replace(" ", "-")}-${scheme}.png`, [s.page.locator(".editor-note")]);
        expect(s.problems).toEqual([]);
        await s.context.close();
      } finally {
        await destroyWorld(fresh);
      }
    });
  }
}
