//! One entry per harness: the files it reads, relative to the login's home folder.
//!
//! Phase 1 uses this to show what each harness sees. Phase 3 adds a pure format per
//! harness (desired entries + current bytes → new bytes) next to each layout.

use atlas_core::kernel::HarnessKind;

/// Where a harness looks, relative to `$HOME`.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub kind: HarnessKind,
    /// Folders it loads skills from, in the order it reads them.
    pub skill_dirs: &'static [&'static str],
    /// Its global instruction files.
    pub instruction_files: &'static [&'static str],
    /// The config file that holds its hooks, if it has hooks.
    pub hook_config: Option<&'static str>,
    /// A path whose presence means the harness is set up for this login.
    pub marker: &'static str,
}

const LAYOUTS: [Layout; 6] = [
    Layout {
        kind: HarnessKind::ClaudeCode,
        skill_dirs: &[".claude/skills"],
        instruction_files: &[".claude/CLAUDE.md"],
        hook_config: Some(".claude/settings.json"),
        marker: ".claude",
    },
    Layout {
        kind: HarnessKind::Codex,
        skill_dirs: &[".agents/skills", ".codex/skills"],
        instruction_files: &[".codex/AGENTS.md"],
        hook_config: Some(".codex/hooks.json"),
        marker: ".codex",
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
    },
    Layout {
        kind: HarnessKind::Pi,
        skill_dirs: &[".pi/agent/skills"],
        instruction_files: &[".pi/agent/AGENTS.md"],
        hook_config: None,
        marker: ".pi/agent",
    },
    Layout {
        kind: HarnessKind::Gemini,
        skill_dirs: &[".gemini/config/skills"],
        instruction_files: &[".gemini/GEMINI.md"],
        hook_config: None,
        marker: ".gemini",
    },
    // T3 Code runs the other harnesses and reads their files.
    Layout {
        kind: HarnessKind::T3,
        skill_dirs: &[],
        instruction_files: &[],
        hook_config: None,
        marker: ".t3",
    },
];

#[must_use]
pub fn layouts() -> &'static [Layout] {
    &LAYOUTS
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
    fn paths_are_relative_and_stay_in_home() {
        for l in &LAYOUTS {
            let all = l
                .skill_dirs
                .iter()
                .chain(l.instruction_files)
                .chain(l.hook_config.iter());
            for p in all.chain([&l.marker]) {
                assert!(
                    !p.starts_with('/') && !p.split('/').any(|s| s == ".."),
                    "{p}"
                );
            }
        }
    }
}
