# Design

Approved direction (2026-11-26): https://claude.ai/artifact/8CMa3Poe5mLiZ6ASsyj6QV

| Token | Value | Use |
|---|---|---|
| limestone | `#ECEAE3` | ground (night edition: `#0E1220`) |
| ink | `#16181D` | text, rules, primary buttons |
| verdigris | `#2E6B5E` | accent, links, "on" |
| gilt | `#B08A3E` | ornament, numerals |
| oxblood | `#7A2430` | drift, errors |

Type: Cinzel (titles, nav, buttons), EB Garamond (body), IBM Plex Mono (paths, labels,
status). Motifs: double rules, a gilt Greek key under the masthead, Roman-numeral
sections, numbered "plates", film grain, ink code blocks with a gilt hairline,
and a flat 2D engraved medallion logo of the crouching verdigris Atlas statue bearing a
gilt armillary sphere (`assets/logo/`, served in `atlas-web` as `/static/logo.png` and
`/static/favicon.png`).

## Density & attention (2026-11-27)
The look stays; the space goes to content. Approved after the owner found v1 wasteful.
Treat reading and attention as currency: avoid reader fatigue by surfacing only what is
needed to scan at a glance and tucking extra text behind an intentional expand or click.
- Rail 188px, collapsible to 52px (remembered per browser). Phones get a top bar.
- Masthead is one running head, an 8px Greek key and one row: 28px title plus the scope
  picker. No subtitles or explanatory paragraphs on pages.
- Rows 46px: 16px name, marks and a chevron, with no plate numbers, location or status
  columns; problems show as a short oxblood flag on the row. On Skills, collapsed rows show
  only the name (no one-line description under each skill); the description or outside
  source appears inside the inline drawer when expanded.
- Agent marks are the real brand marks in one colour (ink, gilt for "all agents"), 16px.
- Detail opens inline (gilt left rule, deeper limestone); full pages only for reading a
  file.
- Notices are marginalia: a gilt left bar and italic text, never a coloured banner.
- Words for non-technical owners: changes reach machines as "updates", never "releases";
  Save sends them with one passkey tap (decision 0022).
