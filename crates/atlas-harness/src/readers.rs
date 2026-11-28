//! Who reads which instruction file: each agent's project and home instruction files, how
//! it picks among them when several exist, and what it reads of other agents' folders.
//! Data plus two pure questions over it ([`project_access`], [`home_access`]); the caller
//! says which files exist. Sources per fact are in `HARNESSES.md` ("Who reads which
//! instruction file").
//!
//! Only whole files in a project's top folder and in the login's home are modelled. Rule
//! folders (`.claude/rules`, `.grok/rules`, …) and files an agent loads through its own
//! config (`OpenCode`'s `instructions`, Codex's fallback names) are out of scope. Claude
//! Code's `@path` import is the only one that inlines another file, so only Claude reads
//! through an import.

use std::collections::{BTreeMap, BTreeSet};

use atlas_core::catalog::ClaudeAgentsMd;
use atlas_core::kernel::HarnessKind;

/// What an agent does when several of its files exist side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Selection {
    /// Every file that exists loads.
    All,
    /// Only the first existing file of the folder loads.
    First,
}

/// A project file name an agent reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectFile {
    /// Relative to the project's top folder.
    pub name: &'static str,
    /// Claude Code reads it only as its AGENTS.md setting allows.
    pub governed: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct ProjectRules {
    /// In the order the agent prefers them.
    pub files: &'static [ProjectFile],
    pub selection: Selection,
    /// It passes over files git ignores (Grok Build), which `CLAUDE.local.md` is meant to be.
    pub skips_ignored: bool,
}

/// How the global `AGENTS.md` reaches the agent's home file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// The home file is a link to it.
    Link,
    /// The home file has an `@<path>` line importing it (Claude Code).
    Import,
}

/// Another agent's home file this agent reads as well.
#[derive(Clone, Copy, Debug)]
pub struct AlsoReads {
    /// Relative to the home folder.
    pub file: &'static str,
    /// Only when none of the agent's own home files exists.
    pub only_without_own: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HomeRules {
    /// Relative to the home folder, in the order the agent prefers them.
    pub files: &'static [&'static str],
    pub selection: Selection,
    /// The file Atlas links or imports the global instructions into.
    pub shared: &'static str,
    pub join: Join,
    pub also_reads: &'static [AlsoReads],
}

#[derive(Clone, Copy, Debug)]
pub struct Readers {
    pub kind: HarnessKind,
    pub project: Option<ProjectRules>,
    pub home: Option<HomeRules>,
}

const fn f(name: &'static str) -> ProjectFile {
    ProjectFile {
        name,
        governed: false,
    }
}

const fn governed(name: &'static str) -> ProjectFile {
    ProjectFile {
        name,
        governed: true,
    }
}

const READERS: [Readers; 8] = [
    Readers {
        kind: HarnessKind::ClaudeCode,
        project: Some(ProjectRules {
            files: &[
                f("CLAUDE.md"),
                f(".claude/CLAUDE.md"),
                f("CLAUDE.local.md"),
                governed("AGENTS.md"),
                governed(".claude/AGENTS.md"),
            ],
            selection: Selection::All,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[".claude/CLAUDE.md"],
            selection: Selection::All,
            shared: ".claude/CLAUDE.md",
            join: Join::Import,
            also_reads: &[],
        }),
    },
    Readers {
        kind: HarnessKind::Codex,
        project: Some(ProjectRules {
            files: &[f("AGENTS.override.md"), f("AGENTS.md")],
            selection: Selection::First,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[".codex/AGENTS.override.md", ".codex/AGENTS.md"],
            selection: Selection::First,
            shared: ".codex/AGENTS.md",
            join: Join::Link,
            also_reads: &[],
        }),
    },
    Readers {
        kind: HarnessKind::OpenCode,
        project: Some(ProjectRules {
            files: &[f("AGENTS.md"), f("CLAUDE.md")],
            selection: Selection::First,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[".config/opencode/AGENTS.md"],
            selection: Selection::First,
            shared: ".config/opencode/AGENTS.md",
            join: Join::Link,
            also_reads: &[AlsoReads {
                file: ".claude/CLAUDE.md",
                only_without_own: true,
            }],
        }),
    },
    Readers {
        kind: HarnessKind::Pi,
        project: Some(ProjectRules {
            files: &[
                f("AGENTS.override.md"),
                f("AGENTS.md"),
                f("AGENTS.MD"),
                f("CLAUDE.md"),
                f("CLAUDE.MD"),
            ],
            selection: Selection::First,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[
                ".pi/agent/AGENTS.override.md",
                ".pi/agent/AGENTS.md",
                ".pi/agent/AGENTS.MD",
                ".pi/agent/CLAUDE.md",
                ".pi/agent/CLAUDE.MD",
            ],
            selection: Selection::First,
            shared: ".pi/agent/AGENTS.md",
            join: Join::Link,
            also_reads: &[],
        }),
    },
    Readers {
        kind: HarnessKind::Antigravity,
        project: Some(ProjectRules {
            files: &[
                f("AGENTS.md"),
                f("GEMINI.md"),
                f(".agents/AGENTS.md"),
                f(".agents/GEMINI.md"),
            ],
            selection: Selection::All,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[".gemini/config/AGENTS.md"],
            selection: Selection::All,
            shared: ".gemini/config/AGENTS.md",
            join: Join::Link,
            also_reads: &[],
        }),
    },
    Readers {
        kind: HarnessKind::Amp,
        project: Some(ProjectRules {
            files: &[f("AGENTS.md"), f("AGENT.md"), f("CLAUDE.md")],
            selection: Selection::First,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[".config/amp/AGENTS.md", ".config/AGENTS.md"],
            selection: Selection::All,
            shared: ".config/amp/AGENTS.md",
            join: Join::Link,
            also_reads: &[],
        }),
    },
    Readers {
        kind: HarnessKind::Muse,
        project: Some(ProjectRules {
            files: &[f("AGENTS.md"), f("CLAUDE.md")],
            selection: Selection::First,
            skips_ignored: false,
        }),
        home: Some(HomeRules {
            files: &[".config/muse/AGENTS.md"],
            selection: Selection::First,
            shared: ".config/muse/AGENTS.md",
            join: Join::Link,
            also_reads: &[],
        }),
    },
    Readers {
        kind: HarnessKind::Grok,
        project: Some(ProjectRules {
            files: &[
                f("Agents.md"),
                f("Claude.md"),
                f("CLAUDE.md"),
                f("CLAUDE.local.md"),
                f("AGENT.md"),
                f("AGENTS.md"),
                f(".claude/CLAUDE.md"),
                f(".claude/CLAUDE.local.md"),
            ],
            selection: Selection::All,
            skips_ignored: true,
        }),
        home: Some(HomeRules {
            files: &[".grok/AGENTS.md", ".grok/Agents.md", ".grok/CLAUDE.md"],
            selection: Selection::All,
            shared: ".grok/AGENTS.md",
            join: Join::Link,
            also_reads: &[AlsoReads {
                file: ".claude/CLAUDE.md",
                only_without_own: false,
            }],
        }),
    },
];

/// Every agent's instruction files.
#[must_use]
pub fn readers() -> &'static [Readers] {
    &READERS
}

/// One agent's instruction files; `None` for T3 Code and Herdr, which only run the others.
#[must_use]
pub fn readers_of(kind: HarnessKind) -> Option<&'static Readers> {
    READERS.iter().find(|r| r.kind == kind)
}

/// Every name some agent reads in a project's top folder.
#[must_use]
pub fn project_names() -> BTreeSet<&'static str> {
    READERS
        .iter()
        .filter_map(|r| r.project)
        .flat_map(|p| p.files.iter().map(|f| f.name))
        .collect()
}

/// What sits at one instruction file's name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileFacts {
    /// A link to another name in the same folder (`CLAUDE.md` → `AGENTS.md`).
    pub links_to: Option<String>,
    /// Names in the same folder it imports with `@<name>` lines (Claude Code only).
    pub imports: BTreeSet<String>,
    /// Git ignores it.
    pub ignored: bool,
}

/// The instruction files that exist in one folder, by name.
pub type Present = BTreeMap<String, FileFacts>;

/// How one agent reaches one file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Access {
    /// It reads the file under its own name.
    Direct,
    /// It reads the named file, which is a link to this one.
    Link(String),
    /// It reads the named file, which imports this one.
    Import(String),
    /// Claude Code reads it because of its AGENTS.md setting.
    Setting,
    None(Why),
}

impl Access {
    #[must_use]
    pub const fn reads(&self) -> bool {
        !matches!(self, Self::None(_))
    }
}

/// Why an agent doesn't read a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Why {
    /// It doesn't read files of this name.
    NotItsName,
    /// It reads only the first of its names, and this one exists before it.
    Earlier(String),
    /// Claude's setting is "never".
    SettingOff,
    /// Claude's setting reads `AGENTS.md` only without these Claude files.
    ClaudeFiles(String),
    /// It skips files git ignores.
    Ignored,
    /// It has no file of this kind at all.
    NoFiles,
}

/// How `kind` reaches the project file `name`, given the files in the project's top folder.
/// `setting` is Claude Code's AGENTS.md option on this machine.
#[must_use]
pub fn project_access(
    kind: HarnessKind,
    name: &str,
    present: &Present,
    setting: ClaudeAgentsMd,
) -> Access {
    let Some(rules) = readers_of(kind).and_then(|r| r.project) else {
        return Access::None(Why::NoFiles);
    };
    // Through a file it reads that links to or imports this one.
    let via = || {
        rules
            .files
            .iter()
            .filter(|other| other.name != name)
            .filter(|other| matches!(direct(kind, &rules, other.name, present, setting), Ok(())))
            .find_map(|other| {
                let facts = present.get(other.name)?;
                if facts.links_to.as_deref() == Some(name) {
                    Some(Access::Link(other.name.to_owned()))
                } else if kind == HarnessKind::ClaudeCode && facts.imports.contains(name) {
                    Some(Access::Import(other.name.to_owned()))
                } else {
                    None
                }
            })
    };
    match direct(kind, &rules, name, present, setting) {
        Ok(()) => {
            let file = rules.files.iter().find(|f| f.name == name);
            if file.is_some_and(|f| f.governed) {
                // An import from a Claude file wins over the setting: it works on any
                // version and whatever the setting says.
                via().unwrap_or(Access::Setting)
            } else {
                Access::Direct
            }
        }
        Err(why) => via().unwrap_or(Access::None(why)),
    }
}

/// Whether `kind` reads `name` itself, or why not.
fn direct(
    kind: HarnessKind,
    rules: &ProjectRules,
    name: &str,
    present: &Present,
    setting: ClaudeAgentsMd,
) -> Result<(), Why> {
    let Some(pos) = rules.files.iter().position(|f| f.name == name) else {
        return Err(Why::NotItsName);
    };
    let file = rules.files[pos];
    if rules.skips_ignored && present.get(name).is_some_and(|f| f.ignored) {
        return Err(Why::Ignored);
    }
    if rules.selection == Selection::First
        && let Some(earlier) = rules.files[..pos].iter().find(|e| {
            present
                .get(e.name)
                .is_some_and(|f| !(rules.skips_ignored && f.ignored))
        })
    {
        return Err(Why::Earlier(earlier.name.to_owned()));
    }
    if file.governed && kind == HarnessKind::ClaudeCode {
        match setting {
            ClaudeAgentsMd::Never => return Err(Why::SettingOff),
            ClaudeAgentsMd::Alongside => {}
            ClaudeAgentsMd::WhenNoClaudeMd => {
                let blocking = rules
                    .files
                    .iter()
                    .filter(|f| !f.governed)
                    .find(|f| present.contains_key(f.name));
                if let Some(b) = blocking {
                    return Err(Why::ClaudeFiles(b.name.to_owned()));
                }
            }
        }
    }
    Ok(())
}

/// What one home file is, relative to the global `AGENTS.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeFile {
    /// A link to the global file.
    LinksGlobal,
    /// Imports the global file (`@<path>`); `own` when it also has text of its own.
    ImportsGlobal { own: bool },
    /// The agent's own text.
    Own,
    /// Something else Atlas doesn't follow (a link elsewhere, an unreadable file).
    Other,
}

/// How `kind` reaches the global `AGENTS.md` on a machine, given what sits at each home
/// file that exists (paths relative to the home folder).
#[must_use]
pub fn home_access(kind: HarnessKind, files: &BTreeMap<String, HomeFile>) -> Access {
    let Some(home) = readers_of(kind).and_then(|r| r.home) else {
        return Access::None(Why::NoFiles);
    };
    let own: Vec<&str> = home
        .files
        .iter()
        .copied()
        .filter(|f| files.contains_key(*f))
        .collect();
    let read: Vec<&str> = match home.selection {
        Selection::First => own.first().copied().into_iter().collect(),
        Selection::All => own.clone(),
    };
    let also = home
        .also_reads
        .iter()
        .filter(|a| !a.only_without_own || own.is_empty())
        .map(|a| a.file);
    let through = |file: &str| match files.get(file) {
        Some(HomeFile::LinksGlobal) => Some(Access::Link(file.to_owned())),
        Some(HomeFile::ImportsGlobal { .. }) if kind == HarnessKind::ClaudeCode => {
            Some(Access::Import(file.to_owned()))
        }
        _ => None,
    };
    if let Some(a) = read.iter().copied().chain(also).find_map(through) {
        return a;
    }
    if home.selection == Selection::First
        && let Some(first) = read.first()
        && *first != home.shared
        && files.contains_key(home.shared)
    {
        return Access::None(Why::Earlier((*first).to_owned()));
    }
    Access::None(Why::NoFiles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layouts;

    fn present(names: &[&str]) -> Present {
        names
            .iter()
            .map(|n| ((*n).to_owned(), FileFacts::default()))
            .collect()
    }

    #[test]
    fn every_agent_with_instructions_has_readers_matching_its_layout() {
        for l in layouts() {
            let r = readers_of(l.kind);
            assert_eq!(r.is_some(), !l.instruction_files.is_empty(), "{}", l.kind);
            let Some(r) = r else { continue };
            let home = r.home.unwrap();
            // atlasd links or imports into exactly the file the table says.
            assert_eq!(l.instruction_files[0], home.shared, "{}", l.kind);
            assert!(home.files.contains(&home.shared));
            for p in l.project_instructions {
                assert!(
                    r.project.unwrap().files.iter().any(|f| f.name == *p),
                    "{} {p}",
                    l.kind
                );
            }
        }
    }

    #[test]
    fn claude_reads_agents_md_only_as_its_setting_says() {
        let only_agents = present(&["AGENTS.md"]);
        let both = present(&["AGENTS.md", "CLAUDE.md"]);
        let claude = HarnessKind::ClaudeCode;
        let d = ClaudeAgentsMd::WhenNoClaudeMd;
        assert_eq!(
            project_access(claude, "AGENTS.md", &only_agents, d),
            Access::Setting
        );
        assert_eq!(
            project_access(claude, "AGENTS.md", &both, d),
            Access::None(Why::ClaudeFiles("CLAUDE.md".into()))
        );
        assert_eq!(
            project_access(claude, "AGENTS.md", &both, ClaudeAgentsMd::Alongside),
            Access::Setting
        );
        assert_eq!(
            project_access(claude, "AGENTS.md", &only_agents, ClaudeAgentsMd::Never),
            Access::None(Why::SettingOff)
        );
        assert_eq!(
            project_access(claude, "CLAUDE.md", &both, d),
            Access::Direct
        );
    }

    #[test]
    fn a_claude_md_that_imports_or_links_agents_md_reaches_it() {
        let mut p = present(&["AGENTS.md", "CLAUDE.md"]);
        p.get_mut("CLAUDE.md")
            .unwrap()
            .imports
            .insert("AGENTS.md".into());
        let claude = HarnessKind::ClaudeCode;
        assert_eq!(
            project_access(claude, "AGENTS.md", &p, ClaudeAgentsMd::Never),
            Access::Import("CLAUDE.md".into())
        );
        let mut p = present(&["AGENTS.md", "CLAUDE.md"]);
        p.get_mut("CLAUDE.md").unwrap().links_to = Some("AGENTS.md".into());
        assert_eq!(
            project_access(claude, "AGENTS.md", &p, ClaudeAgentsMd::WhenNoClaudeMd),
            Access::Link("CLAUDE.md".into())
        );
        // Codex doesn't read CLAUDE.md, so the link doesn't matter to it.
        assert_eq!(
            project_access(HarnessKind::Codex, "AGENTS.md", &p, ClaudeAgentsMd::Never),
            Access::Direct
        );
    }

    #[test]
    fn first_name_agents_skip_later_names() {
        let both = present(&["AGENTS.md", "CLAUDE.md"]);
        let d = ClaudeAgentsMd::WhenNoClaudeMd;
        assert_eq!(
            project_access(HarnessKind::OpenCode, "CLAUDE.md", &both, d),
            Access::None(Why::Earlier("AGENTS.md".into()))
        );
        assert_eq!(
            project_access(
                HarnessKind::OpenCode,
                "CLAUDE.md",
                &present(&["CLAUDE.md"]),
                d
            ),
            Access::Direct
        );
        assert_eq!(
            project_access(HarnessKind::Codex, "CLAUDE.md", &both, d),
            Access::None(Why::NotItsName)
        );
        // Grok reads every file, but not one git ignores.
        assert_eq!(
            project_access(HarnessKind::Grok, "CLAUDE.md", &both, d),
            Access::Direct
        );
        let mut local = present(&["CLAUDE.local.md"]);
        local.get_mut("CLAUDE.local.md").unwrap().ignored = true;
        assert_eq!(
            project_access(HarnessKind::Grok, "CLAUDE.local.md", &local, d),
            Access::None(Why::Ignored)
        );
    }

    #[test]
    fn home_access_follows_links_imports_and_overrides() {
        let files = |e: &[(&str, HomeFile)]| -> BTreeMap<String, HomeFile> {
            e.iter().map(|(n, f)| ((*n).to_owned(), *f)).collect()
        };
        assert_eq!(
            home_access(
                HarnessKind::Codex,
                &files(&[(".codex/AGENTS.md", HomeFile::LinksGlobal)])
            ),
            Access::Link(".codex/AGENTS.md".into())
        );
        assert_eq!(
            home_access(
                HarnessKind::Codex,
                &files(&[
                    (".codex/AGENTS.override.md", HomeFile::Own),
                    (".codex/AGENTS.md", HomeFile::LinksGlobal)
                ])
            ),
            Access::None(Why::Earlier(".codex/AGENTS.override.md".into()))
        );
        let claude = files(&[(".claude/CLAUDE.md", HomeFile::ImportsGlobal { own: false })]);
        assert_eq!(
            home_access(HarnessKind::ClaudeCode, &claude),
            Access::Import(".claude/CLAUDE.md".into())
        );
        // OpenCode reads Claude's file without its own, but doesn't follow imports.
        assert_eq!(
            home_access(HarnessKind::OpenCode, &claude),
            Access::None(Why::NoFiles)
        );
        let linked = files(&[(".claude/CLAUDE.md", HomeFile::LinksGlobal)]);
        assert_eq!(
            home_access(HarnessKind::Grok, &linked),
            Access::Link(".claude/CLAUDE.md".into())
        );
    }
}
