// Keyboard only: from the top of a page, Tab reaches the skip link, every section in the
// nav and the page's own controls, each with a visible focus ring; an edit is saved and a
// passkey sign-in started without a pointer. Every nav item and icon button has a name.

import { test, expect, shot } from "../../lib/owner.mjs";
import { git } from "../../lib/world.mjs";
import { SCREENS, settle, strangerPage } from "../../lib/screens.mjs";

test.use({ worldOptions: { purpose: "a11y" } });

const SECTIONS = ["Skills", "Instructions", "Memory", "Hooks", "Machines", "MCP"];

/** What has focus now: its name, where it is, and whether a focus ring shows. */
const focused = (page) =>
  page.evaluate(() => {
    const el = document.activeElement;
    const css = getComputedStyle(el);
    const box = el.getBoundingClientRect();
    return {
      tag: el.tagName.toLowerCase(),
      text: (el.getAttribute("aria-label") || el.getAttribute("title") || el.textContent || "").trim(),
      href: el.getAttribute("href"),
      inMain: !!el.closest("main"),
      inNav: !!el.closest("nav"),
      ring: css.outlineStyle !== "none" && parseFloat(css.outlineWidth) >= 1,
      onScreen: box.width > 0 && box.height > 0 && box.right > 0 && box.left < innerWidth,
    };
  });

/** Presses Tab until `match` has focus (at most `max` presses); returns the stops. */
async function tabTo(page, match, max = 40) {
  const stops = [];
  for (let i = 0; i < max; i++) {
    await page.keyboard.press("Tab");
    const f = await focused(page);
    stops.push(f);
    if (match(f)) return stops;
  }
  throw new Error(`Tab never reached the target; stops: ${stops.map((s) => s.text || s.tag).join(" → ")}`);
}

test("Tab reaches the skip link, the nav and the page, with a visible focus ring", async ({ owner }) => {
  await owner.goto("/skills");
  await settle(owner);

  await owner.keyboard.press("Tab");
  const skip = await focused(owner);
  expect(skip.text).toBe("Skip to content");
  expect(skip.onScreen, "the skip link shows when focused").toBe(true);
  expect(skip.ring).toBe(true);
  await shot(owner, "skip link focused");

  // Every section's link takes focus in order, each with a ring.
  const stops = await tabTo(owner, (f) => f.inNav && f.href === "/mcp");
  for (const name of SECTIONS) {
    const stop = stops.find((s) => s.inNav && s.text.toLowerCase().includes(name.toLowerCase()));
    expect(stop, `${name} is reachable by Tab`).toBeTruthy();
    expect(stop.ring, `${name} shows a focus ring`).toBe(true);
  }
  await shot(owner, "nav link focused");

  // The page's own controls follow: the scope picker and the filter.
  const main = await tabTo(owner, (f) => f.inMain);
  expect(main.at(-1).ring).toBe(true);
  await shot(owner, "first control in main focused");
  await tabTo(owner, (f) => f.inMain && f.tag === "input");
  await shot(owner, "filter focused");

  // The skip link jumps straight past the nav.
  await owner.goto("/skills");
  await owner.keyboard.press("Tab");
  await owner.keyboard.press("Enter");
  await expect(owner).toHaveURL(/#main$/);
  await owner.keyboard.press("Tab");
  expect((await focused(owner)).inMain, "Tab after the skip link lands in main").toBe(true);
});

test("a skill is opened, edited and saved by keyboard alone", async ({ owner, world }) => {
  const before = git(world, world.knowledge, "rev-parse", "HEAD");
  await owner.goto("/skills");
  await settle(owner);

  // Open the pdf-tools row and follow its link, all from the keyboard.
  await tabTo(owner, (f) => f.tag === "summary" && f.text.includes("pdf-tools"));
  await owner.keyboard.press("Enter");
  await tabTo(owner, (f) => f.text === "Read the skill");
  await shot(owner, "drawer open, link focused");
  await owner.keyboard.press("Enter");
  await expect(owner).toHaveURL(/\/skills\/private\/pdf-tools$/);

  await tabTo(owner, (f) => f.text === "Edit pdf-tools");
  await owner.keyboard.press("Enter");
  await expect(owner).toHaveURL(/\/edit$/);

  await tabTo(owner, (f) => f.tag === "textarea");
  await owner.keyboard.press("Control+End");
  await owner.keyboard.type("\nSaved from the keyboard.\n");
  await owner.keyboard.press("Shift+Tab");
  const save = await focused(owner);
  expect(save.text).toBe("Save");
  expect(save.ring).toBe(true);
  await shot(owner, "save focused");
  await owner.keyboard.press("Enter");
  await expect(owner).toHaveURL(/\/skills\/private\/pdf-tools(\?|$)/);
  await shot(owner, "saved");

  expect(git(world, world.knowledge, "rev-parse", "HEAD")).not.toBe(before);
  expect(git(world, world.knowledge, "show", "HEAD:skills/pdf-tools/SKILL.md")).toContain("Saved from the keyboard.");
});

test("a passkey sign-in is started by keyboard alone", async ({ owner, auth, browser, world }, info) => {
  const s = await strangerPage(browser, world, info, await auth.credentials());
  try {
    await s.page.goto("/signin?next=/memory");
    await settle(s.page);
    await tabTo(s.page, (f) => f.text === "Sign in with a passkey");
    expect((await focused(s.page)).ring).toBe(true);
    await shot(s.page, "sign-in button focused");
    await s.page.keyboard.press("Enter");
    await s.page.waitForURL((u) => u.pathname === "/memory");
    await shot(s.page, "signed in");
    expect(s.problems).toEqual([]);
  } finally {
    await s.context.close();
  }
});

test("every nav item and icon button has an accessible name", async ({ owner }) => {
  for (const screen of SCREENS) {
    await owner.goto(screen.path);
    for (const el of await owner.locator("nav a, button, [role=img]").all()) {
      if (!(await el.isVisible())) continue;
      await expect(el, `${screen.path}: ${await el.evaluate((e) => e.outerHTML.slice(0, 120))}`).toHaveAccessibleName(/\S/);
    }
  }
});

test("Settings and sign-out can be reached from every page", async ({ owner }, info) => {
  if (info.project.name === "phone") {
    info.annotations.push({
      type: "bug",
      description:
        "On phones the rail footer (Settings, sign out) is display:none (atlas.css @media max-width 720px .rail-foot), and nothing else links to /settings or signs out, so a phone owner can't reach Settings, passkeys, CLI tokens or sign out.",
    });
    test.fail();
  }
  await owner.goto("/skills");
  await shot(owner, "skills");
  await expect(owner.getByRole("link", { name: /Settings/ })).toBeVisible();
  await expect(owner.getByRole("button", { name: "Sign out", exact: true })).toBeVisible();
});
