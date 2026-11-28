// Accessibility: axe-core on every screen in the day and night editions. Serious and
// critical violations fail; lesser ones are attached to the report for reading.

import AxeBuilder from "@axe-core/playwright";
import { test, expect, shot } from "../../lib/owner.mjs";
import { SCHEMES, SCREENS, settle, strangerPage } from "../../lib/screens.mjs";

test.use({ worldOptions: { purpose: "a11y" } });

// Known product bugs, by axe rule and the elements it flags. When every serious or critical
// violation on a screen is one of these, the test is marked as an expected failure with
// the bug annotated; any other violation fails it outright. The assertion never changes.
const KNOWN = [
  {
    rule: "label-content-name-mismatch",
    target: /\.foot-link/,
    bug: "Rail footer link shows \"Signed in\" but is named \"Settings and passkeys\" (aria-label), so voice users can't say what they see (WCAG 2.5.3). base.html .foot-link.",
  },
  {
    rule: "color-contrast",
    target: /\.num\b/,
    bug: "Gilt numerals fail text contrast: #9A7632 on limestone is 3.48:1 (section and tile numerals), and the active nav numeral #B08A3E on #ECEAE3 is 2.66:1 (needs 4.5:1). atlas.css .num / .nav a.active.",
  },
  {
    rule: "color-contrast",
    target: /\.active > \.label/,
    bug: "The MCP nav item (data-later, --rail-muted #9C988C) keeps its muted colour when active, on the #ECEAE3 active background: 2.39:1. atlas.css .nav a[data-later].",
  },
  {
    rule: "list",
    target: /\.agent-states/,
    bug: "ul.agent-states holds <li role=\"img\">, which strips the list items, so the list has no items for screen readers. skill.html / instruction.html.",
  },
];

async function audit(page, label) {
  const result = await new AxeBuilder({ page }).analyze();
  await test.info().attach(`axe ${label}`, {
    body: JSON.stringify(result.violations, null, 2),
    contentType: "application/json",
  });
  const found = [];
  for (const v of result.violations.filter((v) => v.impact === "serious" || v.impact === "critical"))
    for (const n of v.nodes) found.push({ rule: v.id, target: n.target.join(" "), text: `${v.impact} ${v.id}: ${v.help} — ${n.target.join(" ")}` });
  const bugs = new Set();
  const unknown = found.filter((f) => {
    const k = KNOWN.find((b) => b.rule === f.rule && b.target.test(f.target));
    if (k) bugs.add(k.bug);
    return !k;
  });
  if (found.length && !unknown.length) {
    for (const description of bugs) test.info().annotations.push({ type: "bug", description });
    test.fail();
  }
  return found.map((f) => f.text);
}

for (const scheme of SCHEMES) {
  for (const screen of SCREENS) {
    const key = `${screen.name} (${scheme})`;
    test(`axe: ${key}`, async ({ owner }) => {
      await owner.emulateMedia({ colorScheme: scheme });
      await owner.goto(screen.path);
      await settle(owner);
      await shot(owner, key);
      expect(await audit(owner, key)).toEqual([]);
    });
  }

  test(`axe: sign-in (${scheme})`, async ({ owner, browser, world }, info) => {
    const s = await strangerPage(browser, world, info);
    try {
      await s.page.emulateMedia({ colorScheme: scheme });
      await s.page.goto("/signin");
      await settle(s.page);
      await shot(s.page, `sign-in (${scheme})`);
      expect(await audit(s.page, `sign-in (${scheme})`)).toEqual([]);
    } finally {
      await s.context.close();
    }
  });
}
