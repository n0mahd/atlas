//! One entry per harness: the files it reads, in the login's home folder and inside a
//! project repo, plus the pure formats atlasd uses to write them (desired entries +
//! current state → what the file should become). Nothing here does I/O.
//!
//! Where each fact comes from (checked on an installed harness, or only documented) is
//! recorded in `HARNESSES.md` next to this crate's `Cargo.toml`.

pub mod change;
pub mod hooks;
pub mod instructions;
pub mod placement;
pub mod skills;
pub mod switches;

use std::path::{Path, PathBuf};

pub use change::{Change, FormatError};

use atlas_core::kernel::HarnessKind;

/// Where a harness looks. Home paths are relative to `$HOME`, project paths to the repo.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub kind: HarnessKind,
    /// Folders it loads global skills from.
    pub skill_dirs: &'static [&'static str],
    /// Its global instruction files.
    pub instruction_files: &'static [&'static str],
    /// The config file that holds its hooks, if it has hooks, or a folder whose `*.json`
    /// files each hold some (Grok Build).
    pub hook_config: Option<&'static str>,
    /// A path whose presence means the harness is set up for this login.
    pub marker: &'static str,
    /// Folders inside a repo it loads project skills from.
    pub project_skill_dirs: &'static [&'static str],
    /// Instruction files inside a repo it reads, first match wins.
    pub project_instructions: &'static [&'static str],
}

const LAYOUTS: [Layout; 10] = [
    Layout {
        kind: HarnessKind::ClaudeCode,
        skill_dirs: &[".claude/skills"],
        instruction_files: &[".claude/CLAUDE.md"],
        hook_config: Some(".claude/settings.json"),
        marker: ".claude",
        project_skill_dirs: &[".claude/skills"],
        // Claude Code reads AGENTS.md only when the repo has no CLAUDE.md.
        project_instructions: &["CLAUDE.md", "AGENTS.md"],
    },
    Layout {
        kind: HarnessKind::Codex,
        skill_dirs: &[".agents/skills", ".codex/skills"],
        instruction_files: &[".codex/AGENTS.md"],
        hook_config: Some(".codex/hooks.json"),
        marker: ".codex",
        project_skill_dirs: &[".agents/skills"],
        project_instructions: &["AGENTS.md"],
    },
    Layout {
        kind: HarnessKind::OpenCode,
        skill_dirs: &[
            ".config/opencode/skills",
            ".claude/skills",
            ".agents/skills",
        ],
        instruction_files: &[".config/opencode/AGENTS.md"],
        hook_config: None,
        marker: ".config/opencode",
        project_skill_dirs: &[".opencode/skills", ".claude/skills", ".agents/skills"],
        project_instructions: &["AGENTS.md", "CLAUDE.md"],
    },
    Layout {
        kind: HarnessKind::Pi,
        skill_dirs: &[".pi/agent/skills"],
        instruction_files: &[".pi/agent/AGENTS.md"],
        hook_config: None,
        marker: ".pi/agent",
        project_skill_dirs: &[".pi/skills", ".agents/skills"],
        project_instructions: &["AGENTS.md", "CLAUDE.md"],
    },
    // Paths from T3 Code's Antigravity driver.
    Layout {
        kind: HarnessKind::Antigravity,
        skill_dirs: &[".gemini/config/skills", ".gemini/antigravity-cli/skills"],
        instruction_files: &[".gemini/config/AGENTS.md"],
        hook_config: Some(".gemini/config/hooks.json"),
        marker: ".gemini",
        project_skill_dirs: &[".gemini/skills", ".agents/skills", ".agent/skills"],
        project_instructions: &["AGENTS.md", "GEMINI.md"],
    },
    // Paths from Amp's manual (ampcode.com/docs: Skills, AGENTS.md, Configuration), in
    // Amp's own precedence order. Hooks are TypeScript plugins, not a command config.
    // MCP servers: `amp.mcpServers` in `.config/amp/settings.json` (decision 0021).
    Layout {
        kind: HarnessKind::Amp,
        skill_dirs: &[
            ".config/agents/skills",
            ".agents/skills",
            ".config/amp/skills",
            ".claude/skills",
        ],
        instruction_files: &[".config/amp/AGENTS.md", ".config/AGENTS.md"],
        hook_config: None,
        marker: ".config/amp",
        project_skill_dirs: &[".agents/skills", ".claude/skills"],
        project_instructions: &["AGENTS.md", "AGENT.md", "CLAUDE.md"],
    },
    // Paths checked against Muse Code 1.4.25 itself (decision 0021). Its hooks use Claude
    // Code's shape under `hooks` in settings.json, next to `mcpServers`.
    Layout {
        kind: HarnessKind::Muse,
        skill_dirs: &[
            ".config/muse/skills",
            ".agents/skills",
            ".claude/skills",
            ".codex/skills",
        ],
        instruction_files: &[".config/muse/AGENTS.md"],
        hook_config: Some(".config/muse/settings.json"),
        marker: ".config/muse",
        project_skill_dirs: &[".agents/skills", ".claude/skills", ".codex/skills"],
        project_instructions: &["AGENTS.md", "CLAUDE.md"],
    },
    // Paths checked against Grok Build 1.0.92 itself with `grok inspect` (decision 0022), in
    // the order it lists them. It loads every instruction file it finds, not just the first;
    // its hooks are a folder of JSON files in Claude Code's shape. MCP servers sit under
    // `[mcp_servers]` in `.grok/config.toml`.
    Layout {
        kind: HarnessKind::Grok,
        skill_dirs: &[
            ".grok/skills",
            ".agents/skills",
            ".claude/skills",
            ".cursor/skills",
        ],
        instruction_files: &[".grok/AGENTS.md"],
        hook_config: Some(".grok/hooks"),
        marker: ".grok",
        project_skill_dirs: &[
            ".grok/skills",
            ".agents/skills",
            ".claude/skills",
            ".cursor/skills",
        ],
        project_instructions: &["AGENTS.md", "AGENT.md", "CLAUDE.md", "CLAUDE.local.md"],
    },
    // T3 Code runs the other harnesses and reads their files.
    Layout {
        kind: HarnessKind::T3,
        skill_dirs: &[],
        instruction_files: &[],
        hook_config: None,
        marker: ".t3",
        project_skill_dirs: &[],
        project_instructions: &[],
    },
    // Herdr is a terminal workspace for the other harnesses, like T3 Code; its
    // `config.toml` holds layout and keys, not skills, instructions or MCP servers.
    Layout {
        kind: HarnessKind::Herdr,
        skill_dirs: &[],
        instruction_files: &[],
        hook_config: None,
        marker: ".config/herdr",
        project_skill_dirs: &[],
        project_instructions: &[],
    },
];

#[must_use]
pub fn layouts() -> &'static [Layout] {
    &LAYOUTS
}

/// Claude Code's own memory folder for a project: `~/.claude/projects/<path>/memory`,
/// where `<path>` is the repo's absolute path with every other character turned into `-`.
#[must_use]
pub fn claude_project_memory(home: &Path, repo: &Path) -> PathBuf {
    let key: String = repo
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    home.join(".claude/projects").join(key).join("memory")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_harness_has_exactly_one_layout() {
        for kind in HarnessKind::ALL {
            assert_eq!(
                LAYOUTS.iter().filter(|l| l.kind == kind).count(),
                1,
                "{kind}"
            );
        }
    }

    #[test]
    fn paths_are_relative_and_stay_inside() {
        for l in &LAYOUTS {
            let all = l
                .skill_dirs
                .iter()
                .chain(l.instruction_files)
                .chain(l.hook_config.iter())
                .chain(l.project_skill_dirs)
                .chain(l.project_instructions);
            for p in all.chain([&l.marker]) {
                assert!(
                    !p.starts_with('/') && !p.split('/').any(|s| s == ".."),
                    "{p}"
                );
            }
        }
    }

    #[test]
    fn harness_facts_table_covers_every_home_path() {
        let doc = include_str!("../HARNESSES.md");
        for l in &LAYOUTS {
            for p in l
                .skill_dirs
                .iter()
                .chain(l.instruction_files)
                .chain(l.hook_config.iter())
            {
                assert!(doc.contains(&format!("`{p}`")), "HARNESSES.md lacks {p}");
            }
        }
    }

    #[test]
    fn claude_memory_folder_matches_claude_code() {
        assert_eq!(
            claude_project_memory(Path::new("/home/u"), Path::new("/home/u/Projects/my.app")),
            PathBuf::from("/home/u/.claude/projects/-home-u-Projects-my-app/memory")
        );
    }
}
