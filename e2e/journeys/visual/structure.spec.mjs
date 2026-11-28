// Structural style checks that pixels alone can miss. A baseline records whatever is on
// screen, so a stylesheet that silently stops applying on desktop (an unclosed @media
// block turns every later rule into a phone-only rule) gets blessed when baselines are
// re-recorded. These checks fail on that regardless of baselines.

import { test, expect } from "../../lib/owner.mjs";
import { SCREENS } from "../../lib/screens.mjs";

test.use({ worldOptions: { purpose: "visual" } });

/**
 * Computed styles every screen must have at desktop width. `when` entries apply only where
 * the control exists; add one for each new kind of control (see SKILL.md, Maintain).
 */
const ALWAYS = [
  { sel: ".rail", check: (s) => parseFloat(s.width) >= 150, what: "rail is a side column" },
  { sel: "body", check: (s) => s.backgroundImage !== "none" || s.backgroundColor !== "rgba(0, 0, 0, 0)", what: "body has a ground" },
];
const WHEN_PRESENT = [
  { sel: ".button-ink", check: (s) => s.borderTopStyle !== "none", what: "ink buttons have a border" },
  { sel: ".nav a", check: (s) => s.display !== "inline", what: "nav links are blocks" },
  { sel: ".toggle", check: (s) => s.borderTopStyle !== "none", what: "toggles have a border" },
  { sel: ".toggles", check: (s) => s.listStyleType === "none", what: "toggle lists have no bullets" },
];

test("the stylesheet has no @media nested in @media", async ({ owner }, info) => {
  test.skip(info.project.name === "phone", "the stylesheet is the same at every width");
  await owner.goto("/");
  const nested = await owner.evaluate(() => {
    const out = [];
    for (const sheet of document.styleSheets) {
      for (const rule of sheet.cssRules) {
        if (rule instanceof CSSMediaRule)
          for (const inner of rule.cssRules)
            if (inner instanceof CSSMediaRule) out.push(`${rule.conditionText} > ${inner.conditionText}`);
      }
    }
    return out;
  });
  expect(nested, "an unclosed @media block nests every later block inside it").toEqual([]);
});

for (const screen of SCREENS) {
  test(`desktop styles apply: ${screen.name}`, async ({ owner }, info) => {
    test.skip(info.project.name === "phone", "this guards desktop-width styles");
    await owner.setViewportSize({ width: 1280, height: 900 });
    await owner.goto(screen.path);
    for (const probe of [...ALWAYS, ...WHEN_PRESENT]) {
      const el = owner.locator(probe.sel).first();
      if (!(await el.count())) {
        expect(ALWAYS.includes(probe), `${probe.sel} missing on ${screen.path}`).toBe(false);
        continue;
      }
      const style = await el.evaluate((e) => {
        const s = getComputedStyle(e);
        return {
          width: s.width,
          display: s.display,
          borderTopStyle: s.borderTopStyle,
          listStyleType: s.listStyleType,
          backgroundImage: s.backgroundImage,
          backgroundColor: s.backgroundColor,
        };
      });
      expect(probe.check(style), `${probe.what} on ${screen.path}: ${JSON.stringify(style)}`).toBe(true);
    }
  });
}
