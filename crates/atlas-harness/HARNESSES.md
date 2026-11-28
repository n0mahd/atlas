# Harness facts

What `src/lib.rs` (`LAYOUTS`) and the formats in `src/hooks/` rely on, and where each
fact comes from. Home paths are relative to `$HOME`.

- `verified`: checked on an installed copy of the harness (its own config files, the
  strings and embedded docs in its binary, or behaviour seen in a session).
- `docs`: taken from the harness's documentation only.

Checked 2026-11-27 against Claude Code 2.1.332, Codex CLI 0.172.0, Antigravity CLI
1.5.18, Muse Code 1.4.25 (decision 0021) and Grok Build 1.0.92 (decision 0022). Amp needs a
sign-in to run, so its rows are `docs`. OpenCode and Pi were not installed on the machine
used, so their rows are `docs`.

| Harness | Skills | Instruction file | Hooks |
|---|---|---|---|
| Claude Code | `.claude/skills` · verified | `.claude/CLAUDE.md`, with `@<path>` imports · verified | `.claude/settings.json` key `hooks` · verified |
| Codex | `.agents/skills`, `.codex/skills` · verified | `.codex/AGENTS.md` · verified | `.codex/hooks.json` · verified (file); format and events · docs |
| OpenCode | `.config/opencode/skills`, `.claude/skills`, `.agents/skills` · docs | `.config/opencode/AGENTS.md` · docs | none in a config file: JS/TS plugins in `.config/opencode/plugins/` · docs |
| Pi | `.pi/agent/skills` · docs | `.pi/agent/AGENTS.md` · docs | none in a config file: TypeScript extensions in `.pi/agent/extensions/` · docs |
| Antigravity | `.gemini/config/skills`, `.gemini/antigravity-cli/skills` · verified (first), T3 driver (second) | `.gemini/config/AGENTS.md` · verified | `.gemini/config/hooks.json` · verified |
| Amp | `.config/agents/skills`, `.agents/skills`, `.config/amp/skills`, `.claude/skills` · docs | `.config/amp/AGENTS.md` and `.config/AGENTS.md` · docs | none in a config file: TypeScript plugins in `.config/amp/plugins/` · docs |
| Muse | `.config/muse/skills`, `.agents/skills`, `.claude/skills`, `.codex/skills` · verified | `.config/muse/AGENTS.md` · verified | `.config/muse/settings.json` key `hooks`, Claude Code's shape · verified |
| Grok Build | `.grok/skills`, `.agents/skills`, `.claude/skills`, `.cursor/skills` · verified | `.grok/AGENTS.md` (also `.grok/Agents.md`, `.grok/CLAUDE.md`, `.grok/rules/*.md`, `.claude/CLAUDE.md`, `.claude/rules/*.md`) · verified | `.grok/hooks`, a folder of `*.json` files in Claude Code's shape; also runs `.claude/settings.json` hooks · verified |
| T3 Code | none: runs the harnesses above and reads their files | none | none |
| Herdr | none: a terminal workspace that runs other harnesses | none | none of its own; `herdr integration install` adds status hooks to other harnesses · verified |

## Subagents

| Harness | Subagent definitions |
|---|---|
| Claude Code | `.claude/agents`, one `<name>.md` (YAML frontmatter + prompt) per subagent · docs (code.claude.com/docs/en/sub-agents) |
| Every other harness | not handled yet: Atlas writes no subagents for them until each one's format is checked |

atlasd links `<knowledge>/agents/<name>.md` into `.claude/agents` with the same rules as
skill links (decision 0028).

## Hook formats

| Harness | Shape | Atlas's entries | Events (Atlas → harness) |
|---|---|---|---|
| Claude Code | `hooks → Event → [{matcher, hooks: [{type, command, timeout}]}]` | handler `command` ends in the shell comment `# atlas:<name>` | session-start → `SessionStart`, user-prompt → `UserPromptSubmit`, pre-tool-use → `PreToolUse`, post-tool-use → `PostToolUse`, stop → `Stop` |
| Codex | same as Claude Code | same as Claude Code | same names as Claude Code |
| Muse | same as Claude Code | same as Claude Code | same names as Claude Code |
| Grok Build | same as Claude Code, one or more files per folder; `type` may also be `http` with a `url` | `.grok/hooks/atlas.json`, holding only hooks Claude Code's copy doesn't already bring (decision 0026) | Claude Code's names plus `SessionEnd`, `PostToolUseFailure`, `PermissionDenied`, `StopFailure`, `Notification`, `SubagentStart`/`Stop`, `PreCompact`/`PostCompact` |
| Antigravity | `{"<hook name>": {Event: [...]}}`; tool events use matcher groups, others list handlers directly | top-level key `atlas:<name>` | session-start, pre-tool-use, post-tool-use, stop; user-prompt has no equivalent and is skipped |

Notes behind the table:

- Claude Code runs a `command` without `args` through `sh -c` ("shell form"), so a
  trailing `# …` is a comment. Its docs list the handler fields (`type`, `command`,
  `args`, `shell`, `timeout`, `if`, `statusMessage`, `once`, …) but don't say whether
  unknown keys are accepted, and an invalid entry is skipped with a Settings Warning,
  so Atlas adds no extra keys. Source: code.claude.com/docs/en/hooks and /settings.
- Codex reads `$CODEX_HOME/hooks.json` (also `[hooks]` inline in `config.toml`; Atlas
  uses the JSON file only). Hooks not from managed config must be trusted before they
  run; the trust record is a hash under `hooks.state` in `config.toml`, so a changed
  Atlas hook needs the user to trust it again. Source: learn.chatgpt.com/docs/hooks
  (formerly developers.openai.com/codex/hooks) and the 0.172.0 binary.
- Antigravity runs `command` via `sh -c`, with timeout in seconds (default 30). Its
  hook stdin/stdout payloads are camelCase protojson, not Claude Code's, so a script
  written for one harness doesn't necessarily work in the other. Its tool names differ
  too (`run_command`, not `Bash`), so a Claude-style matcher won't match there. Source:
  the docs embedded in the 1.5.18 binary.
- Grok Build: every path above is from `grok inspect --json` on 1.0.92 with a throwaway
  home holding probe files (linked skill folders and a linked `AGENTS.md` included), plus
  the user guide it unpacks into `~/.grok/docs/` and docs.x.ai/build. It reads no
  `~/.grok/GROK.md`, `~/.codex/skills` or `~/.config/agents/skills`. Project skills,
  instructions and hooks load only once the folder is trusted (`~/.grok/trusted_folders.toml`).
  Hook `timeout` is in seconds, default 5; Claude tool names in a `matcher` are mapped to
  Grok's. Whether it expands Claude's `@<path>` imports was not established, as that needs
  a signed-in session; its docs don't mention them.

## Skill switches

What `src/switches/` writes so an agent that reads a skill's folder leaves the skill
alone (decision 0027). Paths are relative to `$HOME` (home) or the repo root (repo); where
two files are listed, they are in the agent's precedence order: the first that exists is
edited, and the first is created when neither does (`SwitchFile::create`). "T3" is
the owner's T3 Code (`apps/server/src/skills/*SkillSettings.ts`, branch
`feat/skills-instructions`), which checked these against the CLIs it names.

| Harness | File | Setting | Atlas's entries | Source |
|---|---|---|---|---|
| Claude Code | home `.claude/settings.json`; repo `.claude/settings.local.json` | `"skillOverrides": {"<folder name>": "off"}` | names listed under top-level `"atlas:skillOverrides"` | verified: `claude doctor` on 2.1.332 accepts the extra key, reads the repo's local file and rejects a value outside `on`, `name-only`, `user-invocable-only`, `off`; docs (skills, settings); T3 |
| Codex | home `.codex/config.toml`, for home and project skills alike | `[[skills.config]]` with `path = "<dir>/SKILL.md"` (or, home only, `name = "<name>"`) and `enabled = false` | header line ends `# atlas:<name>` | verified: 0.172.0 app-server `skills/list` with a throwaway `CODEX_HOME` |
| OpenCode | home `.config/opencode/opencode.jsonc`, else `.config/opencode/opencode.json`; no repo file | `"permission": {"skill": {"<name>": "deny"}}` | entry line ends `// atlas:<name>` | verified: `opencode debug config` on 2.4.2 with a throwaway home reads Atlas's edited `.jsonc` golden file and a `.json` Atlas created, comments and all, with the rules in file order, and leaves both files as they were; OpenCode source (`config/config.ts`, `config/paths.ts`, `config/parse.ts` at 4ac0d9c3d1); docs (opencode.ai/docs/skills); T3 checked opencode 1.18.31 |
| Pi | home `.pi/agent/settings.json`; no repo file | `"skills": ["-skills/<name>/SKILL.md"]` | names listed under top-level `"atlas:skills"` | Pi source (`core/package-manager.ts`, `core/settings-manager.ts` at 43d3763991); docs (pi.dev/docs/latest/settings); T3. Not run: Pi isn't installed here |
| Antigravity, Amp, Muse, Grok Build | none | no per-skill setting in their docs (T3 found the same for Antigravity and Grok) | reported as `stuck_on` | docs; T3 |

Every format marks Atlas's entries in the file itself, so atlasd's own record of what it
switched off (`atlas_before`) isn't needed to tell them apart, and an owner's entry made
before Atlas wanted the same skill off is never taken for Atlas's. An entry of the owner's
for a skill, on or off, is left as it is and Atlas adds none beside it.

Notes behind the table:

- Claude Code names a skill by its folder, not the `name` in its header (T3, checked on
  the CLI). Its settings files are strict JSON (docs: "a `//` comment or a trailing comma
  is a syntax error"), which is why Atlas can't mark an entry with a comment and keeps a
  list instead. One unknown value makes Claude drop the whole `skillOverrides` map, so
  Atlas refuses a file holding one. Not verified: that Claude Code keeps the extra key
  when it rewrites a settings file itself (the `/skills` menu writes
  `.claude/settings.local.json`); if it ever drops it, Atlas's entries read as the
  owner's and stay off until removed by hand.
- Codex, checked on 0.172.0 with a throwaway home: a path entry and a name entry each
  switch the skill off; a `path` through a link to the skill's real folder works (Codex
  resolves it); its own `skills/config/write` keeps the `# atlas:` header comments. The
  last entry naming a skill wins (T3). A project's `.codex/config.toml` is not read for
  skill rules in an untrusted project (T3), so a project's skills are switched off by
  their absolute path in the home file. A name entry would switch off every skill of that
  name, so Atlas writes one only for a home skill whose folder it wasn't given. Home and
  project switches share the file, so it is rendered once with every name off there.
- OpenCode reads `opencode.json` and `opencode.jsonc` as JSONC (comments and trailing
  commas allowed), in the global folder `config.json`, `opencode.json`, then
  `opencode.jsonc`, merging `permission` objects; rules are evaluated last-match-wins
  (T3, from `permission/index.ts`), so Atlas adds its entries at the end. OpenCode creates
  `opencode.jsonc` for a new global config, edits a `.jsonc` in place (keeping comments)
  but rewrites a global `.json` with `JSON.stringify` when it saves settings itself;
  that would drop Atlas's `// atlas:` marks, leaving its entries to read as the owner's.
  So Atlas edits the `.jsonc` when both exist (it is merged last, so its rules win) and
  creates a `.jsonc` when neither does.
  It adds a `$schema` line to any config file lacking one, so a file Atlas creates
  already has it, after a comment saying Atlas made it. The rule key is the `name` in
  the skill's header, not its folder (T3). A repo's `opencode.json` is usually tracked
  and shared, so Atlas writes no repo file.
- Pi applies the global `skills` list to skills found in `~/.pi/agent/skills` and
  `~/.agents/skills`, matching an exact `-`/`+` entry against the skill's `SKILL.md` or
  folder, relative to either base or absolute; `-` beats `+`. It reads the file with
  `JSON.parse` and, when saving, rewrites only the fields it changed, keeping other keys.
  `PI_CODING_AGENT_DIR` moves the folder (not handled by Atlas). A repo's
  `.pi/settings.json` is usually committed, so Atlas has no repo switch for Pi.

## Who reads which instruction file

What `src/readers.rs` (`READERS`) holds: the files each agent reads in a project's top
folder and in the login's home, and how it picks when several exist. **All**: every file
that exists loads. **First**: only the first existing name loads, so a later name is
skipped while an earlier one exists. Atlas joins the global `AGENTS.md` at the home file
named under "Global joins" (a link, or for Claude Code an `@<path>` import). Claude Code's
`@path` import is the only one that inlines another file. Most selection rules come from
the owner's T3 Code table (`AgentInstructionFiles.ts`, T3 Code PR 17515), which cites each
agent's docs and source; marked here as `T3 table` with that source.

| Agent | Project files (in order) | Home files | Global joins | Source |
|---|---|---|---|---|
| Claude Code | All: `CLAUDE.md`, `.claude/CLAUDE.md`, `CLAUDE.local.md`; `AGENTS.md` and `.claude/AGENTS.md` only as its AGENTS.md setting says | `.claude/CLAUDE.md` | import | code.claude.com/docs/en/memory (T3 table); the setting verified in the 2.1.332 binary (below) |
| Codex | First: `AGENTS.override.md`, `AGENTS.md` | First non-empty: `.codex/AGENTS.override.md`, `.codex/AGENTS.md` | link | learn.chatgpt.com/docs/agent-configuration/agents-md and codex `core/src/agents_md.rs` (T3 table) |
| OpenCode | First: `AGENTS.md`, then `CLAUDE.md` | `.config/opencode/AGENTS.md`; without it, `.claude/CLAUDE.md` (imports not followed) | link | opencode.ai/docs/rules and `session/instruction.ts` (T3 table) |
| Pi | First: `AGENTS.override.md`, `AGENTS.md`, `AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD` | the same five names in `.pi/agent/` | link | pi `docs/configuration.md` and `core/resource-loader.ts` (T3 table) |
| Antigravity | All: `AGENTS.md`, `GEMINI.md`, `.agents/AGENTS.md`, `.agents/GEMINI.md` | `.gemini/config/AGENTS.md` | link | antigravity.google/docs/rules (T3 table); home file verified (above) |
| Amp | First: `AGENTS.md`, `AGENT.md`, `CLAUDE.md` | All: `.config/amp/AGENTS.md`, `.config/AGENTS.md` | link | ampcode.com/docs (AGENTS.md) · docs |
| Muse | First: `AGENTS.md`, `CLAUDE.md` | `.config/muse/AGENTS.md` | link | Muse Code 1.4.25 (names verified; that it picks the first is from its layout, not checked) |
| Grok Build | All, except files git ignores: `Agents.md`, `Claude.md`, `CLAUDE.md`, `CLAUDE.local.md`, `AGENT.md`, `AGENTS.md`, `.claude/CLAUDE.md`, `.claude/CLAUDE.local.md` | All: `.grok/AGENTS.md`, `.grok/Agents.md`, `.grok/CLAUDE.md`, and always `.claude/CLAUDE.md` | link | `grok inspect` on 1.0.92 (verified, above) and grok-build `xai-grok-config/src/compat.rs` (T3 table) |

Claude Code's AGENTS.md setting: `pluginConfigs["cc-plugin-agents-md@builtin"].options.instructionFiles`
in `~/.claude/settings.json` (ignored in project settings), one of `claude-md-or-agents-md`
(the default: AGENTS.md only where a project has no CLAUDE.md, `.claude/CLAUDE.md` or
`CLAUDE.local.md`), `claude-md-and-agents-md` (beside them), `claude-md` (never) and
`managed-only`. Before 2.1.285 the plugin was `agents-md@builtin`; later versions read
either, so Atlas keeps a legacy entry that has a value in step. Claude Code reads AGENTS.md
at all from 2.1.277. Source: the T3 table's `ClaudeInstructionSetting.ts` (citing
code.claude.com/docs/en/memory, "Choose which instruction files load"); the plugin name,
the `instructionFiles` option and the four values verified in the strings of the Claude
Code 2.1.332 binary, which describes it as "AGENTS.md as project instructions: by default
loaded where the project has no CLAUDE.md; by its instructionFiles option, loaded beside
CLAUDE.md, left out, or with the project instructions dropped". The `@builtin` suffix is
from the T3 table only.

Not checked: whether Amp reads both of its home files (atlasd links both, which would
load the global file twice), and whether Grok Build expands Claude's `@<path>` imports in
`~/.claude/CLAUDE.md` (the table assumes not, so Grok reaches Global only through a link).
