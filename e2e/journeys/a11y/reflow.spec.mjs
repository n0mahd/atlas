// Reflow (WCAG 1.4.10): at 200% zoom and at 320 CSS px wide, no screen scrolls sideways
// and no control is cut off at the edge. 200% zoom of the desktop window is the same CSS
// width as half of it; on a phone 200% would be ~206px, below the 320px WCAG floor.

import { test, expect, shot } from "../../lib/owner.mjs";
import { SCREENS, settle } from "../../lib/screens.mjs";

test.use({ worldOptions: { purpose: "a11y" } });

/** Visible controls that stick out past the left or right edge of the window. */
const clipped = (page) =>
  page.evaluate(() => {
    const out = [];
    for (const el of document.querySelectorAll("a, button, input, select, textarea, summary")) {
      const r = el.getBoundingClientRect();
      if (!r.width || !r.height || getComputedStyle(el).visibility === "hidden") continue;
      if (el.closest(".sr, .skip")) continue;
      if (r.left < -1 || r.right > innerWidth + 1)
        out.push(`${el.closest(".rail") ? "nav " : ""}${el.tagName.toLowerCase()} "${(el.getAttribute("aria-label") || el.textContent).trim().slice(0, 40)}" at ${Math.round(r.left)}–${Math.round(r.right)} of ${innerWidth}`);
    }
    return out;
  });

const sideways = (page) =>
  page.evaluate(() => ({ scroll: document.documentElement.scrollWidth, client: document.documentElement.clientWidth }));

for (const zoom of ["200%", "320px"]) {
  test(`reflow at ${zoom}`, async ({ owner }, info) => {
    const full = info.project.use.viewport;
    test.skip(zoom === "200%" && info.project.name === "phone", "200% of a phone is ~206px, below the 320px WCAG reflow floor; covered by the 320px test");
    const width = zoom === "200%" ? Math.floor(full.width / 2) : 320;
    await owner.setViewportSize({ width, height: zoom === "200%" ? Math.floor(full.height / 2) : 640 });
    const problems = [];
    try {
      for (const screen of SCREENS) {
        await owner.goto(screen.path);
        await settle(owner);
        await shot(owner, `${screen.name} at ${zoom}`);
        const { scroll, client } = await sideways(owner);
        if (scroll > client) problems.push(`${screen.path}: page scrolls sideways (${scroll} > ${client})`);
        // The top bar scrolls on its own; "the top bar shows every section" covers it.
        for (const c of await clipped(owner)) if (!c.startsWith("nav ")) problems.push(`${screen.path}: ${c}`);
      }
    } finally {
      await owner.setViewportSize(full);
    }
    expect(problems).toEqual([]);
  });
}

test("the top bar shows every section at 320px", async ({ owner }, info) => {
  const full = info.project.use.viewport;
  info.annotations.push({
    type: "bug",
    description:
      "At 320px the top bar overflows: the last section (MCP) is cut off at the right edge (299-327 of 320), with no sign the bar scrolls. atlas.css @media max-width 720px .rail overflow-x: auto. (At 412px it fits.)",
  });
  test.fail();
  await owner.setViewportSize({ width: 320, height: 640 });
  try {
    await owner.goto("/skills");
    await settle(owner);
    await shot(owner, "top bar at 320px");
    const cut = (await clipped(owner)).filter((c) => c.startsWith("nav "));
    expect(cut).toEqual([]);
  } finally {
    await owner.setViewportSize(full);
  }
});
