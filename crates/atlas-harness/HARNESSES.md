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
alone (decision 0027). Paths are relative to `$HOME` (home) or the repo root (repo).
"T3" is the owner's T3 Code (`apps/server/src/skills/*SkillSettings.ts`, branch
`feat/skills-instructions`), which checked these against the CLIs it names.

| Harness | File | Setting | Atlas's entries | Source |
|---|---|---|---|---|
| Claude Code | home `.claude/settings.json`; repo `.claude/settings.local.json` | `"skillOverrides": {"<folder name>": "off"}` | names listed under top-level `"atlas:skillOverrides"` | verified: `claude doctor` on 2.1.332 accepts the extra key, reads the repo's local file and rejects a value outside `on`, `name-only`, `user-invocable-only`, `off`; docs (skills, settings); T3 |
| Codex | home `.codex/config.toml` | `[[skills.config]]` with `path = "<dir>/SKILL.md"` (or `name = "<name>"`) and `enabled = false` | header line ends `# atlas:<name>` | verified: 0.172.0 app-server `skills/list` with a throwaway `CODEX_HOME` |
| OpenCode | home `.config/opencode/opencode.jsonc`; repo `opencode.jsonc` | `"permission": {"skill": {"<name>": "deny"}}` | entry line ends `// atlas:<name>` | OpenCode source (`config/config.ts`, `config/paths.ts`, `config/parse.ts` at 4ac0d9c3d1); docs (opencode.ai/docs/skills); T3 checked opencode 1.18.31 |
| Pi | home `.pi/agent/settings.json` | `"skills": ["-skills/<name>/SKILL.md"]` | names listed under top-level `"atlas:skills"` | Pi source (`core/package-manager.ts`, `core/settings-manager.ts` at 43d3763991); docs (pi.dev/docs/latest/settings); T3. Not run: Pi isn't installed here |
| Antigravity, Amp, Muse, Grok Build | none | no per-skill setting in their docs (T3 found the same for Antigravity and Grok) | reported as `stuck_on` | docs; T3 |

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
  skill rules in an untrusted project (T3), so Atlas has no repo switch for Codex.
- OpenCode reads `opencode.json` and `opencode.jsonc` as JSONC (comments and trailing
  commas allowed), in the global folder `config.json`, `opencode.json`, then
  `opencode.jsonc`, and in a repo `opencode.json` before `opencode.jsonc`, merging
  `permission` objects; rules are evaluated last-match-wins (T3, from
  `permission/index.ts`). Atlas writes `opencode.jsonc`: it is read last, OpenCode
  creates it for a new global config, and OpenCode edits `.jsonc` in place (keeping
  comments) but rewrites a global `.json` with `JSON.stringify`. OpenCode adds a
  `$schema` line to any config file lacking one, so a file Atlas creates already has it.
  The rule key is the `name` in the skill's header, not its folder (T3).
- Pi applies the global `skills` list to skills found in `~/.pi/agent/skills` and
  `~/.agents/skills`, matching an exact `-`/`+` entry against the skill's `SKILL.md` or
  folder, relative to either base or absolute; `-` beats `+`. It reads the file with
  `JSON.parse` and, when saving, rewrites only the fields it changed, keeping other keys.
  `PI_CODING_AGENT_DIR` moves the folder (not handled by Atlas). A repo's
  `.pi/settings.json` is usually committed, so Atlas has no repo switch for Pi.
