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
| Grok Build | same as Claude Code, one or more files per folder; `type` may also be `http` with a `url` | Atlas writes none: Claude Code's copy already runs there | Claude Code's names plus `SessionEnd`, `PostToolUseFailure`, `PermissionDenied`, `StopFailure`, `Notification`, `SubagentStart`/`Stop`, `PreCompact`/`PostCompact` |
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
