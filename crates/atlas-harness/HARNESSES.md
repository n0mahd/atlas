# Harness facts

What `src/lib.rs` (`LAYOUTS`) and the formats in `src/hooks/` rely on, and where each
fact comes from. Home paths are relative to `$HOME`.

- `verified`: checked on an installed copy of the harness (its own config files, the
  strings and embedded docs in its binary, or behaviour seen in a session).
- `docs`: taken from the harness's documentation only.

Checked 2026-11-27 against Claude Code 2.1.332, Codex CLI 0.172.0 and Antigravity CLI
1.5.18. OpenCode and Pi were not installed on the machine used, so their rows are
`docs`.

| Harness | Skills | Instruction file | Hooks |
|---|---|---|---|
| Claude Code | `.claude/skills` · verified | `.claude/CLAUDE.md`, with `@<path>` imports · verified | `.claude/settings.json` key `hooks` · verified |
| Codex | `.agents/skills`, `.codex/skills` · verified | `.codex/AGENTS.md` · verified | `.codex/hooks.json` · verified (file); format and events · docs |
| OpenCode | `.config/opencode/skills`, `.claude/skills`, `.agents/skills` · docs | `.config/opencode/AGENTS.md` · docs | none in a config file: JS/TS plugins in `.config/opencode/plugins/` · docs |
| Pi | `.pi/agent/skills` · docs | `.pi/agent/AGENTS.md` · docs | none in a config file: TypeScript extensions in `.pi/agent/extensions/` · docs |
| Antigravity | `.gemini/config/skills`, `.gemini/antigravity-cli/skills` · verified (first), T3 driver (second) | `.gemini/config/AGENTS.md` · verified | `.gemini/config/hooks.json` · verified |
| T3 Code | none: runs the harnesses above and reads their files | none | none |

## Hook formats

| Harness | Shape | Atlas's entries | Events (Atlas → harness) |
|---|---|---|---|
| Claude Code | `hooks → Event → [{matcher, hooks: [{type, command, timeout}]}]` | handler `command` ends in the shell comment `# atlas:<name>` | session-start → `SessionStart`, user-prompt → `UserPromptSubmit`, pre-tool-use → `PreToolUse`, post-tool-use → `PostToolUse`, stop → `Stop` |
| Codex | same as Claude Code | same as Claude Code | same names as Claude Code |
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
