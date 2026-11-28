//! View models: what each page shows, built from one snapshot of the store.

use std::collections::{BTreeSet, HashSet};

use atlas_core::catalog::{Catalog, MemoryKind, MemoryPage, Skill, SkillHome};
use atlas_core::distribution::{HarnessView, InstalledSkill, InstructionLink, ProjectView};
use atlas_core::kernel::{HarnessKind, Scope};

use crate::icons;
use crate::markdown;

/// Everything read for one request.
#[derive(Debug)]
pub struct Snapshot {
    pub catalog: Catalog,
    pub harnesses: Vec<HarnessView>,
    pub projects: Vec<ProjectView>,
    /// What sits outside Atlas on this machine (`atlas_fs::outside`).
    pub outside: atlas_fs::outside::Outside,
}

/// The agents Atlas shows marks for. T3 Code and Herdr run the others, so they have none
/// of their own.
pub const AGENTS: [HarnessKind; 8] = [
    HarnessKind::ClaudeCode,
    HarnessKind::Codex,
    HarnessKind::OpenCode,
    HarnessKind::Pi,
    HarnessKind::Antigravity,
    HarnessKind::Amp,
    HarnessKind::Muse,
    HarnessKind::Grok,
];

/// Short names, as people say them.
const fn short(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::ClaudeCode => "Claude",
        HarnessKind::Codex => "Codex",
        HarnessKind::OpenCode => "OpenCode",
        HarnessKind::Pi => "Pi",
        HarnessKind::Antigravity => "Antigravity",
        HarnessKind::Amp => "Amp",
        HarnessKind::Muse => "Muse",
        HarnessKind::Grok => "Grok",
        HarnessKind::T3 => "T3 Code",
        HarnessKind::Herdr => "Herdr",
    }
}

/// An agent's short name, for pages outside this module.
#[must_use]
pub(crate) const fn agent_name(kind: HarnessKind) -> &'static str {
    short(kind)
}

/// The marks for agents that have something: one sparkles mark when every agent set up
/// here has it.
#[must_use]
pub(crate) fn agent_marks(snap: &Snapshot, have: &[HarnessKind]) -> (Vec<AgentMark>, bool) {
    marks(have, &present_agents(snap))
}

fn join_names(kinds: &[HarnessKind]) -> String {
    let names: Vec<&str> = kinds.iter().map(|&k| short(k)).collect();
    match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

// ---------------------------------------------------------------- page chrome

#[derive(Debug, Clone)]
pub struct NavItem {
    pub num: &'static str,
    pub icon: &'static str,
    pub label: &'static str,
    pub href: String,
    pub active: bool,
    pub later: bool,
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub checked: bool,
    pub disabled: bool,
}

/// What every page's frame shows.
#[derive(Debug, Clone)]
pub struct Page {
    pub title: String,
    pub section: &'static str,
    pub folio: String,
    pub totals: String,
    pub hub: String,
    pub nav: Vec<NavItem>,
    pub problems: usize,
    /// The scope this page shows: `global` or a project name.
    pub scope: String,
    pub scopes: Vec<Choice>,
    pub show_scope: bool,
    pub machines: Vec<Choice>,
    /// Passkey sign-in is set up on this hub.
    pub can_sign_in: bool,
    pub signed_in: bool,
    /// The session's CSRF token, for forms that change something.
    pub csrf: String,
    /// The edit page for what this page shows, if it can be edited.
    pub edit_href: Option<String>,
    /// A one-line notice, such as "Saved and committed as …".
    pub flash: String,
    /// A button the notice offers, such as Send for a save that didn't reach the machines.
    pub flash_action: Option<&'static str>,
}

/// The scope a request asked for, if it names a known project; otherwise global.
#[must_use]
pub fn resolve_scope(snap: &Snapshot, asked: &str) -> Scope {
    snap.projects
        .iter()
        .find(|p| p.project.as_str() == asked)
        .map_or(Scope::Global, |p| Scope::Project(p.project.clone()))
}

fn scope_key(scope: &Scope) -> String {
    scope
        .project()
        .map_or_else(|| "global".to_owned(), ToString::to_string)
}

impl Page {
    /// `section` is the nav path; `scoped` pages carry the scope in links and show the picker.
    #[must_use]
    pub fn new(
        section: &'static str,
        title: impl Into<String>,
        hub: &str,
        others: &[String],
        snap: &Snapshot,
        scope: &Scope,
        scoped: bool,
    ) -> Self {
        let key = scope_key(scope);
        let carry = |href: &str, scoped_page: bool| {
            if scoped_page && key != "global" {
                format!("{href}?scope={key}")
            } else {
                href.to_owned()
            }
        };
        let items = [
            ("I", "Skills", "/skills", true, false),
            ("II", "Instructions", "/instructions", true, false),
            ("III", "Memory", "/memory", true, false),
            ("IV", "Hooks", "/hooks", false, false),
            ("V", "Machines", "/machines", false, false),
            ("VI", "MCP", "/mcp", false, true),
        ];
        let nav: Vec<NavItem> = items
            .into_iter()
            .map(|(num, label, href, scoped_page, later)| NavItem {
                num,
                icon: icons::section(label),
                label,
                href: carry(href, scoped_page),
                active: href == section,
                later,
            })
            .collect();
        let title = title.into();
        let folio = nav.iter().find(|n| n.active).map_or_else(
            || {
                if section == "/" {
                    "Frontispiece".to_owned()
                } else {
                    title.clone()
                }
            },
            |n| format!("Folio {} · {}", n.num, n.label),
        );
        let mut scope_choices = vec![Choice {
            value: "global".into(),
            label: "Global".into(),
            checked: key == "global",
            disabled: false,
        }];
        scope_choices.extend(snap.projects.iter().map(|p| Choice {
            value: p.project.to_string(),
            label: p.project.to_string(),
            checked: p.project.as_str() == key,
            disabled: false,
        }));
        let mut machines = vec![Choice {
            value: hub.to_owned(),
            label: hub.to_owned(),
            checked: true,
            disabled: false,
        }];
        machines.extend(others.iter().map(|m| Choice {
            value: m.clone(),
            label: format!("{m} (not paired)"),
            checked: false,
            disabled: true,
        }));
        Self {
            title,
            section,
            folio,
            totals: String::new(),
            hub: hub.to_owned(),
            nav,
            problems: snap.catalog.problems.len(),
            scope: key,
            scopes: scope_choices,
            show_scope: scoped,
            machines,
            can_sign_in: false,
            signed_in: false,
            csrf: String::new(),
            edit_href: None,
            flash: String::new(),
            flash_action: None,
        }
    }
}

fn matches(q: &str, fields: &[&str]) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || fields.iter().any(|f| f.to_lowercase().contains(&q))
}

fn memory_names(catalog: &Catalog) -> HashSet<&str> {
    catalog.memory.iter().map(|m| m.name.as_str()).collect()
}

fn harness(snap: &Snapshot, kind: HarnessKind) -> Option<&HarnessView> {
    snap.harnesses.iter().find(|h| h.kind == kind)
}

/// Agents set up for this login on the hub.
fn present_agents(snap: &Snapshot) -> Vec<HarnessKind> {
    AGENTS
        .iter()
        .copied()
        .filter(|&k| harness(snap, k).is_some_and(|h| h.present))
        .collect()
}

// ---------------------------------------------------------------- agent marks

#[derive(Debug, Clone)]
pub struct AgentMark {
    pub icon: &'static str,
    pub label: String,
}

/// The marks for a set of agents: one sparkles mark when every agent present has it.
fn marks(have: &[HarnessKind], present: &[HarnessKind]) -> (Vec<AgentMark>, bool) {
    let all = !present.is_empty() && present.iter().all(|k| have.contains(k));
    if all {
        return (
            vec![AgentMark {
                icon: icons::ALL_AGENTS,
                label: "Every agent has it".to_owned(),
            }],
            true,
        );
    }
    let marks = have
        .iter()
        .map(|&k| AgentMark {
            icon: icons::icon(k),
            label: format!("{} has it", short(k)),
        })
        .collect();
    (marks, false)
}

#[derive(Debug, Clone)]
pub struct AgentState {
    pub icon: &'static str,
    pub name: &'static str,
    pub state: String,
    /// `ok`, `warn` or `muted`.
    pub class: &'static str,
}

impl AgentState {
    /// The status badge drawn on the agent's mark.
    #[must_use]
    pub fn badge(&self) -> &'static str {
        match self.class {
            "ok" => icons::BADGE_OK,
            "warn" => icons::BADGE_MISSING,
            _ => icons::BADGE_ABSENT,
        }
    }

    /// Whether the state says more than the badge does, so it's worth showing as text.
    #[must_use]
    pub fn needs_words(&self) -> bool {
        self.class == "warn" && !matches!(self.state.as_str(), "doesn't have it" | "missing")
    }
}

// ---------------------------------------------------------------- overview

#[derive(Debug)]
pub struct Tile {
    pub num: &'static str,
    pub label: &'static str,
    pub count: usize,
    pub detail: String,
    pub href: &'static str,
}

#[derive(Debug)]
pub struct ProblemRow {
    pub path: String,
    pub message: String,
}

#[derive(Debug)]
pub struct Overview {
    pub tiles: Vec<Tile>,
    pub problems: Vec<ProblemRow>,
    pub notices: Vec<Notice>,
}

#[derive(Debug, Clone)]
pub struct Notice {
    pub text: String,
    pub href: String,
    pub action: &'static str,
}

#[must_use]
pub fn overview(snap: &Snapshot) -> Overview {
    let c = &snap.catalog;
    let global = c
        .skills
        .iter()
        .filter(|s| s.home.scope() == Scope::Global)
        .count();
    let store_memory = c.memory.iter().filter(|m| m.scope == Scope::Global).count();
    let found_hooks: usize = snap.harnesses.iter().map(|h| h.hooks.len()).sum();
    let mut notices = Vec::new();
    let outside = crate::import::overview_count(snap);
    if outside > 0 {
        notices.push(Notice {
            text: if outside == 1 {
                "1 skill on this machine isn't in Atlas.".to_owned()
            } else {
                format!("{outside} skills on this machine aren't in Atlas.")
            },
            href: "/skills/tidy".to_owned(),
            action: "Tidy up",
        });
    }
    for p in &snap.projects {
        let n = claude_missing(snap, p);
        if n > 0 {
            notices.push(Notice {
                text: format!("Claude can't use {n} of {}'s skills.", p.project),
                href: format!("/skills?scope={}", p.project),
                action: "Show",
            });
        }
    }
    Overview {
        tiles: vec![
            Tile {
                num: "I",
                label: "Skills",
                count: c.skills.len(),
                detail: format!("{global} global, {} in projects", c.skills.len() - global),
                href: "/skills",
            },
            Tile {
                num: "II",
                label: "Instructions",
                count: c.instructions.len(),
                detail: "files".to_owned(),
                href: "/instructions",
            },
            Tile {
                num: "III",
                label: "Memory",
                count: store_memory,
                detail: "notes in the store".to_owned(),
                href: "/memory",
            },
            Tile {
                num: "IV",
                label: "Hooks",
                count: c.hooks.len() + found_hooks,
                detail: format!("{found_hooks} outside Atlas"),
                href: "/hooks",
            },
        ],
        problems: c
            .problems
            .iter()
            .map(|p| ProblemRow {
                path: p.path.clone(),
                message: p.message.clone(),
            })
            .collect(),
        notices,
    }
}

// ---------------------------------------------------------------- skills

#[derive(Debug, Clone)]
pub struct SkillRow {
    pub name: String,
    pub description: String,
    pub href: Option<String>,
    pub marks: Vec<AgentMark>,
    pub all: bool,
    /// A short warning shown on the row, such as "Not in Pi".
    pub flag: String,
    pub agents: Vec<AgentState>,
    pub path: String,
    pub files: usize,
    /// A project skill with the same name as a global one.
    pub also_global: bool,
}

#[derive(Debug)]
pub struct SkillSection {
    pub id: &'static str,
    pub num: &'static str,
    pub title: String,
    pub rows: Vec<SkillRow>,
    pub notice: Option<String>,
}

#[derive(Debug)]
pub struct SkillsList {
    pub sections: Vec<SkillSection>,
    pub total: usize,
    pub q: String,
}

fn agent_states(have: &[HarnessKind], present: &[HarnessKind]) -> Vec<AgentState> {
    AGENTS
        .iter()
        .map(|&k| {
            let (state, class) = if !present.contains(&k) {
                ("not set up here".to_owned(), "muted")
            } else if have.contains(&k) {
                ("has it".to_owned(), "ok")
            } else {
                ("doesn't have it".to_owned(), "warn")
            };
            AgentState {
                icon: icons::icon(k),
                name: short(k),
                state,
                class,
            }
        })
        .collect()
}

fn global_row(snap: &Snapshot, skill: &Skill, present: &[HarnessKind]) -> SkillRow {
    let have: Vec<HarnessKind> = present
        .iter()
        .copied()
        .filter(|&k| harness(snap, k).is_some_and(|h| h.sees(skill.name.as_str())))
        .collect();
    let missing: Vec<HarnessKind> = present
        .iter()
        .copied()
        .filter(|k| !have.contains(k))
        .collect();
    let (marks, all) = marks(&have, present);
    SkillRow {
        name: skill.name.to_string(),
        description: skill.description.clone(),
        href: Some(format!("/skills/{}/{}", skill.home.key(), skill.name)),
        marks,
        all,
        flag: if have.is_empty() {
            "No agent has it".to_owned()
        } else if missing.is_empty() {
            String::new()
        } else {
            format!("Not in {}", join_names(&missing))
        },
        agents: agent_states(&have, present),
        path: skill.path.clone(),
        files: skill.files.len(),
        also_global: false,
    }
}

fn project_view<'a>(snap: &'a Snapshot, scope: &Scope) -> Option<&'a ProjectView> {
    let p = scope.project()?;
    snap.projects.iter().find(|v| &v.project == p)
}

fn project_have(view: &ProjectView, name: &str, present: &[HarnessKind]) -> Vec<HarnessKind> {
    view.harnesses
        .iter()
        .filter(|h| present.contains(&h.kind) && h.skills.iter().any(|s| s == name))
        .map(|h| h.kind)
        .collect()
}

/// How many of a project's skills Claude Code can't load (they sit only in `.agents/skills`).
fn claude_missing(snap: &Snapshot, view: &ProjectView) -> usize {
    let present = present_agents(snap);
    if !present.contains(&HarnessKind::ClaudeCode) {
        return 0;
    }
    snap.catalog
        .skills
        .iter()
        .filter(|s| s.home == SkillHome::Project(view.project.clone()))
        .filter(|s| {
            !project_have(view, s.name.as_str(), &present).contains(&HarnessKind::ClaudeCode)
        })
        .count()
}

/// The "This project" section: the repo's skills and which agents load them.
fn project_section(snap: &Snapshot, view: &ProjectView, present: &[HarnessKind]) -> SkillSection {
    let present = present.to_vec();
    let home = SkillHome::Project(view.project.clone());
    let rows: Vec<SkillRow> = snap
        .catalog
        .skills
        .iter()
        .filter(|s| s.home == home)
        .map(|s| {
            let have = project_have(view, s.name.as_str(), &present);
            let missing: Vec<HarnessKind> = present
                .iter()
                .copied()
                .filter(|k| !have.contains(k))
                .collect();
            let (marks, all) = marks(&have, &present);
            SkillRow {
                name: s.name.to_string(),
                description: s.description.clone(),
                href: Some(format!("/skills/{}/{}", s.home.key(), s.name)),
                marks,
                all,
                flag: if missing.is_empty() {
                    String::new()
                } else {
                    format!("Not in {}", join_names(&missing))
                },
                agents: agent_states(&have, &present),
                path: s.path.clone(),
                files: s.files.len(),
                also_global: snap
                    .catalog
                    .skills
                    .iter()
                    .any(|g| g.home.scope() == Scope::Global && g.name == s.name),
            }
        })
        .collect();
    let n = claude_missing(snap, view);
    SkillSection {
        id: "project",
        num: "I",
        title: "This project".to_owned(),
        rows,
        notice: (n > 0).then(|| {
            if n == 1 {
                "Claude can't use 1 of this project's skills: it sits only in .agents/skills."
                    .to_owned()
            } else {
                format!("Claude can't use {n} of this project's skills: they sit only in .agents/skills.")
            }
        }),
    }
}

#[must_use]
pub fn skills(snap: &Snapshot, scope: &Scope, q: &str) -> SkillsList {
    let present = present_agents(snap);
    let mut sections = Vec::new();
    let mut total = 0;

    if let Some(view) = project_view(snap, scope) {
        let section = project_section(snap, view, &present);
        total += section.rows.len();
        sections.push(section);
    }

    let global: Vec<SkillRow> = snap
        .catalog
        .skills
        .iter()
        .filter(|s| s.home.scope() == Scope::Global)
        .map(|s| global_row(snap, s, &present))
        .collect();
    total += global.len();
    sections.push(SkillSection {
        id: "global",
        num: if sections.is_empty() { "I" } else { "II" },
        title: "Global".to_owned(),
        rows: global,
        notice: None,
    });

    // Outside Atlas and Built in come from `crate::import::skills_outside`.
    for s in &mut sections {
        s.rows.retain(|r| matches(q, &[&r.name, &r.description]));
    }
    SkillsList {
        sections,
        total,
        q: q.to_owned(),
    }
}

#[derive(Debug)]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub declared_name: Option<String>,
    pub lives_in: String,
    pub path: String,
    pub files: Vec<String>,
    pub agents: Vec<AgentState>,
    pub body_html: String,
}

#[must_use]
pub fn skill_detail(snap: &Snapshot, home: &str, name: &str) -> Option<SkillDetail> {
    let skill = snap
        .catalog
        .skills
        .iter()
        .find(|s| s.home.key() == home && s.name.as_str() == name)?;
    let present = present_agents(snap);
    let agents = match &skill.home {
        SkillHome::Project(p) => {
            let view = snap.projects.iter().find(|v| &v.project == p);
            let have = view.map_or_else(Vec::new, |v| project_have(v, name, &present));
            agent_states(&have, &present)
        }
        _ => global_row(snap, skill, &present).agents,
    };
    Some(SkillDetail {
        name: skill.name.to_string(),
        description: skill.description.clone(),
        declared_name: skill.declared_name.clone(),
        lives_in: match &skill.home {
            SkillHome::Private => "Global, private".to_owned(),
            SkillHome::Public => "Global, public".to_owned(),
            SkillHome::Project(p) => format!("The {p} repo"),
        },
        path: skill.path.clone(),
        files: skill.files.clone(),
        agents,
        body_html: markdown::render(&skill.body, &memory_names(&snap.catalog)),
    })
}

// ---------------------------------------------------------------- instructions

#[derive(Debug)]
pub struct InstructionRow {
    pub name: String,
    pub href: String,
    pub marks: Vec<AgentMark>,
    pub flag: String,
    pub lines: usize,
}

#[derive(Debug)]
pub struct InstructionSection {
    pub num: &'static str,
    pub title: &'static str,
    pub rows: Vec<InstructionRow>,
    pub empty: &'static str,
}

/// Agents whose global instruction file is (or imports) the store's.
fn global_readers(snap: &Snapshot) -> (Vec<HarnessKind>, Vec<HarnessKind>) {
    let present = present_agents(snap);
    let reading: Vec<HarnessKind> = present
        .iter()
        .copied()
        .filter(|&k| {
            harness(snap, k).is_some_and(|h| {
                h.instructions
                    .iter()
                    .any(|i| matches!(i.link, InstructionLink::Linked | InstructionLink::Imports))
            })
        })
        .collect();
    let not: Vec<HarnessKind> = present
        .iter()
        .copied()
        .filter(|k| !reading.contains(k))
        .collect();
    (reading, not)
}

#[must_use]
pub fn instructions(snap: &Snapshot, scope: &Scope) -> Vec<InstructionSection> {
    let present = present_agents(snap);
    let mut sections = Vec::new();
    if let Some(view) = project_view(snap, scope) {
        let rows = snap
            .catalog
            .instructions
            .iter()
            .filter(|f| &f.scope == scope)
            .map(|f| {
                let readers: Vec<HarnessKind> = view
                    .harnesses
                    .iter()
                    .filter(|h| {
                        present.contains(&h.kind) && h.instructions.as_deref() == Some(&f.file_name)
                    })
                    .map(|h| h.kind)
                    .collect();
                let skipped: Vec<HarnessKind> = view
                    .harnesses
                    .iter()
                    .filter(|h| {
                        present.contains(&h.kind)
                            && !readers.contains(&h.kind)
                            && h.instructions.is_some()
                    })
                    .map(|h| h.kind)
                    .collect();
                let (marks, _) = marks(&readers, &present);
                InstructionRow {
                    name: f.file_name.clone(),
                    href: format!("/instructions/{}/{}", view.project, f.file_name),
                    marks,
                    flag: if readers.is_empty() {
                        "No agent reads it".to_owned()
                    } else if skipped.is_empty() {
                        String::new()
                    } else {
                        format!(
                            "{} read{} the other file",
                            join_names(&skipped),
                            if skipped.len() == 1 { "s" } else { "" }
                        )
                    },
                    lines: f.body.raw().as_str().lines().count(),
                }
            })
            .collect();
        sections.push(InstructionSection {
            num: "I",
            title: "This project",
            rows,
            empty: "This repo has no AGENTS.md or CLAUDE.md.",
        });
    }
    let (reading, not) = global_readers(snap);
    let rows = snap
        .catalog
        .instructions
        .iter()
        .filter(|f| f.scope == Scope::Global)
        .map(|f| {
            let (marks, _) = marks(&reading, &present);
            InstructionRow {
                name: f.file_name.clone(),
                href: format!("/instructions/global/{}", f.file_name),
                marks,
                flag: if not.is_empty() {
                    String::new()
                } else {
                    format!("Not used by {}", join_names(&not))
                },
                lines: f.body.raw().as_str().lines().count(),
            }
        })
        .collect();
    sections.push(InstructionSection {
        num: if sections.is_empty() { "I" } else { "II" },
        title: "Global",
        rows,
        empty: "Your knowledge repo has no AGENTS.md yet.",
    });
    sections
}

#[derive(Debug)]
pub struct InstructionDetail {
    pub title: String,
    pub path: String,
    pub agents: Vec<AgentState>,
    pub body_html: String,
}

#[must_use]
pub fn instruction_detail(snap: &Snapshot, scope: &str, file: &str) -> Option<InstructionDetail> {
    let f = snap.catalog.instructions.iter().find(|f| {
        f.file_name == file
            && match &f.scope {
                Scope::Global => scope == "global",
                Scope::Project(p) => p.as_str() == scope,
            }
    })?;
    let present = present_agents(snap);
    let agents = match &f.scope {
        Scope::Global => AGENTS
            .iter()
            .map(|&k| {
                let view = harness(snap, k);
                let (state, class) = match view.and_then(|h| h.instructions.first()) {
                    _ if !present.contains(&k) => ("not set up here".to_owned(), "muted"),
                    Some(i) => match i.link {
                        InstructionLink::Linked => (format!("linked from {}", i.path), "ok"),
                        InstructionLink::Imports => (format!("imported by {}", i.path), "ok"),
                        InstructionLink::Separate => (format!("uses its own {}", i.path), "warn"),
                        InstructionLink::Missing => (format!("no {}", i.path), "warn"),
                    },
                    None => ("not set up here".to_owned(), "muted"),
                };
                AgentState {
                    icon: icons::icon(k),
                    name: short(k),
                    state,
                    class,
                }
            })
            .collect(),
        Scope::Project(p) => {
            let view = snap.projects.iter().find(|v| &v.project == p);
            AGENTS
                .iter()
                .map(|&k| {
                    let reads = view
                        .and_then(|v| v.harnesses.iter().find(|h| h.kind == k))
                        .and_then(|h| h.instructions.clone());
                    let (state, class) = match reads {
                        _ if !present.contains(&k) => ("not set up here".to_owned(), "muted"),
                        Some(r) if r == file => ("reads it".to_owned(), "ok"),
                        Some(r) => (format!("reads {r} instead"), "warn"),
                        None => ("reads neither".to_owned(), "warn"),
                    };
                    AgentState {
                        icon: icons::icon(k),
                        name: short(k),
                        state,
                        class,
                    }
                })
                .collect()
        }
    };
    Some(InstructionDetail {
        title: match &f.scope {
            Scope::Global => format!("Global {}", f.file_name),
            Scope::Project(p) => format!("{p} · {}", f.file_name),
        },
        path: f.path.clone(),
        agents,
        body_html: markdown::render(&f.body, &memory_names(&snap.catalog)),
    })
}

// ---------------------------------------------------------------- memory

#[derive(Debug)]
pub struct MemoryRow {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub kind_class: &'static str,
    pub host: String,
    pub href: String,
}

#[derive(Debug)]
pub struct MemoryList {
    pub rows: Vec<MemoryRow>,
    pub total: usize,
    pub q: String,
    pub kinds: Vec<Choice>,
    pub hosts: Vec<Choice>,
    /// For a project: the agent whose notes these are.
    pub kept_by: Option<AgentMark>,
    pub project: Option<String>,
}

const fn kind_class(kind: &MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Unknown(_) => "warn",
        _ => "plain",
    }
}

fn memory_href(m: &MemoryPage) -> String {
    match &m.scope {
        Scope::Global => format!("/memory/{}", m.name),
        Scope::Project(p) => format!("/memory/{}?scope={p}", m.name),
    }
}

#[must_use]
pub fn memory(snap: &Snapshot, scope: &Scope, q: &str, kind: &str, host: &str) -> MemoryList {
    let pages: Vec<&MemoryPage> = snap
        .catalog
        .memory
        .iter()
        .filter(|m| &m.scope == scope)
        .collect();
    let kind_now = if kind.is_empty() { "all" } else { kind };
    let host_now = if host.is_empty() { "all" } else { host };
    let host_names: BTreeSet<&str> = pages
        .iter()
        .map(|m| m.host.as_str())
        .filter(|h| *h != "all")
        .collect();
    let mut hosts = vec![
        Choice {
            value: "all".into(),
            label: "Every host".into(),
            checked: host_now == "all",
            disabled: false,
        },
        Choice {
            value: "shared".into(),
            label: "Shared".into(),
            checked: host_now == "shared",
            disabled: false,
        },
    ];
    hosts.extend(host_names.into_iter().map(|h| Choice {
        value: h.to_owned(),
        label: h.to_owned(),
        checked: host_now == h,
        disabled: false,
    }));
    let kinds = ["all", "user", "feedback", "project", "reference", "untyped"]
        .iter()
        .map(|k| Choice {
            value: (*k).to_owned(),
            label: if *k == "all" {
                "All".to_owned()
            } else {
                (*k).to_owned()
            },
            checked: *k == kind_now,
            disabled: false,
        })
        .collect();
    let rows = pages
        .iter()
        .filter(|m| kind_now == "all" || m.kind.label() == kind_now)
        .filter(|m| match host_now {
            "all" => true,
            "shared" => m.host == "all",
            h => m.host == h,
        })
        .filter(|m| matches(q, &[&m.name, &m.description]))
        .map(|m| MemoryRow {
            name: m.name.clone(),
            description: m.description.clone(),
            kind: m.kind.label().to_owned(),
            kind_class: kind_class(&m.kind),
            host: if m.host == "all" {
                String::new()
            } else {
                m.host.clone()
            },
            href: memory_href(m),
        })
        .collect();
    MemoryList {
        rows,
        total: pages.len(),
        q: q.to_owned(),
        kinds,
        hosts,
        kept_by: pages.iter().find_map(|m| m.kept_by).map(|k| AgentMark {
            icon: icons::icon(k),
            label: format!("{}'s notes", short(k)),
        }),
        project: scope.project().map(ToString::to_string),
    }
}

#[derive(Debug)]
pub struct LinkRow {
    pub name: String,
    pub exists: bool,
}

#[derive(Debug)]
pub struct MemoryDetail {
    pub name: String,
    pub description: String,
    pub kind: String,
    pub kind_class: &'static str,
    pub host: String,
    pub path: String,
    pub links: Vec<LinkRow>,
    pub backlinks: Vec<String>,
    pub body_html: String,
}

#[must_use]
pub fn memory_detail(snap: &Snapshot, scope: &Scope, name: &str) -> Option<MemoryDetail> {
    let pages = &snap.catalog.memory;
    let page = pages.iter().find(|m| m.name == name && &m.scope == scope)?;
    let names = memory_names(&snap.catalog);
    Some(MemoryDetail {
        name: page.name.clone(),
        description: page.description.clone(),
        kind: page.kind.label().to_owned(),
        kind_class: kind_class(&page.kind),
        host: page.host.clone(),
        path: page.path.clone(),
        links: page
            .links
            .iter()
            .map(|l| LinkRow {
                name: l.clone(),
                exists: names.contains(l.as_str()),
            })
            .collect(),
        backlinks: pages
            .iter()
            .filter(|m| m.links.iter().any(|l| l == name))
            .map(|m| m.name.clone())
            .collect(),
        body_html: markdown::render(&page.body, &names),
    })
}

// ---------------------------------------------------------------- hooks

#[derive(Debug)]
pub struct AtlasHookRow {
    pub name: String,
    pub scope_label: String,
    pub description: String,
    pub event: &'static str,
    pub matcher: String,
    pub run: String,
    pub timeout: String,
    pub harnesses: String,
    pub path: String,
    pub ok: bool,
}

#[derive(Debug)]
pub struct FoundHookRow {
    pub icon: &'static str,
    pub harness: &'static str,
    pub event: String,
    pub matcher: String,
    pub command: String,
    pub timeout: String,
    pub config: String,
}

#[derive(Debug)]
pub struct Hooks {
    pub atlas: Vec<AtlasHookRow>,
    pub found: Vec<FoundHookRow>,
}

#[must_use]
pub fn hooks(snap: &Snapshot) -> Hooks {
    let atlas = snap
        .catalog
        .hooks
        .iter()
        .map(|h| AtlasHookRow {
            name: h.name.to_string(),
            scope_label: match &h.scope {
                Scope::Global => "Global".to_owned(),
                Scope::Project(p) => p.to_string(),
            },
            description: h.manifest.description.clone(),
            event: h.manifest.event.label(),
            matcher: h
                .manifest
                .matcher
                .clone()
                .unwrap_or_else(|| "every tool".to_owned()),
            run: h.manifest.run.clone(),
            timeout: h
                .manifest
                .timeout
                .map_or_else(|| "default".to_owned(), |t| format!("{t}s")),
            harnesses: h.manifest.harnesses.as_ref().map_or_else(
                || "every agent with this event".to_owned(),
                |hs| join_names(hs),
            ),
            path: h.path.clone(),
            ok: h.script_present,
        })
        .collect();
    let found = snap
        .harnesses
        .iter()
        .flat_map(|h| {
            h.hooks.iter().map(move |f| FoundHookRow {
                icon: icons::icon(h.kind),
                harness: short(h.kind),
                event: f.event.clone(),
                matcher: f.matcher.clone().unwrap_or_else(|| "every tool".to_owned()),
                command: mask_secrets(&f.command),
                timeout: f.timeout.map_or_else(String::new, |t| format!("{t}s")),
                config: h.hook_config.clone().unwrap_or_default(),
            })
        })
        .collect();
    Hooks { atlas, found }
}

// ---------------------------------------------------------------- machines

#[derive(Debug)]
pub struct HarnessLine {
    pub icon: &'static str,
    pub name: &'static str,
    /// Skills from Atlas, in ink.
    pub skills: String,
    /// Outside or broken skills, in oxblood; empty when there are none.
    pub skills_warn: String,
    pub instructions: String,
    pub instructions_class: &'static str,
    pub hooks: String,
    pub dirs: Vec<String>,
}

#[derive(Debug)]
pub struct Machine {
    pub name: String,
    pub hub: bool,
    pub summary: String,
    pub harnesses: Vec<HarnessLine>,
    pub absent: Vec<&'static str>,
    /// Clients (one per OS account) paired on this machine.
    pub clients: Vec<PairedClient>,
}

#[derive(Debug)]
pub struct PairedClient {
    pub id: String,
    pub user: String,
    pub status: String,
    pub class: &'static str,
    pub problems: Vec<String>,
}

/// The hub itself, from a live look at its own harness folders.
fn hub_machine(snap: &Snapshot, hub: &str) -> Machine {
    let mut lines = Vec::new();
    let mut absent = Vec::new();
    let outside_total = snap.outside.skills.iter().filter(|s| !s.kept).count();
    for &k in &AGENTS {
        let Some(h) = harness(snap, k) else { continue };
        if !h.present {
            absent.push(short(k));
            continue;
        }
        let entries: Vec<&InstalledSkill> = h.skill_dirs.iter().flat_map(|d| &d.entries).collect();
        let from: BTreeSet<&str> = entries
            .iter()
            .filter(|e| matches!(e, InstalledSkill::FromAtlas { .. }))
            .map(|e| e.name())
            .collect();
        let outside: BTreeSet<&str> = entries
            .iter()
            .filter(|e| matches!(e, InstalledSkill::Outside { .. }))
            .map(|e| e.name())
            .collect();
        let broken = entries
            .iter()
            .filter(|e| matches!(e, InstalledSkill::Broken { .. }))
            .count();
        let mut parts = Vec::new();
        if !outside.is_empty() {
            parts.push(format!("{} outside", outside.len()));
        }
        if broken > 0 {
            parts.push(format!("{broken} broken"));
        }
        let skills_warn = parts.join(", ");
        let (instructions, instructions_class) = match h.instructions.first().map(|i| &i.link) {
            Some(InstructionLink::Linked) => ("Global AGENTS.md linked".to_owned(), "ok"),
            Some(InstructionLink::Imports) => ("Global AGENTS.md imported".to_owned(), "ok"),
            Some(InstructionLink::Separate) => ("Own instructions file".to_owned(), "warn"),
            Some(InstructionLink::Missing) | None => ("No global instructions".to_owned(), "warn"),
        };
        lines.push(HarnessLine {
            icon: icons::icon(k),
            name: short(k),
            skills: format!("{} skills from Atlas", from.len()),
            skills_warn,
            instructions,
            instructions_class,
            hooks: match h.hooks.len() {
                0 => String::new(),
                1 => "1 hook".to_owned(),
                n => format!("{n} hooks"),
            },
            dirs: h
                .skill_dirs
                .iter()
                .filter(|d| d.exists)
                .map(|d| d.path.clone())
                .chain(h.instructions.iter().map(|i| i.path.clone()))
                .chain(h.hook_config.iter().cloned())
                .collect(),
        });
    }
    Machine {
        name: hub.to_owned(),
        hub: true,
        summary: format!(
            "{} agents{}",
            lines.len(),
            if outside_total > 0 {
                format!(" · {outside_total} skills outside Atlas")
            } else {
                String::new()
            }
        ),
        harnesses: lines,
        absent,
        clients: Vec::new(),
    }
}

#[must_use]
pub fn machines(
    snap: &Snapshot,
    hub: &str,
    others: &[String],
    clients: &[crate::clients::Client],
    latest: Option<u64>,
) -> Vec<Machine> {
    let mut out = vec![hub_machine(snap, hub)];
    // Other machines: the configured names plus any machine a client paired from.
    let mut names: Vec<String> = others.to_vec();
    for c in clients {
        if c.machine != hub && !names.contains(&c.machine) {
            names.push(c.machine.clone());
        }
    }
    out.extend(names.into_iter().map(|name| Machine {
        summary: String::new(),
        harnesses: Vec::new(),
        absent: Vec::new(),
        clients: Vec::new(),
        hub: false,
        name,
    }));
    for m in &mut out {
        m.clients = clients
            .iter()
            .filter(|c| c.machine == m.name)
            .map(|c| paired(c, latest))
            .collect();
        if !m.hub {
            if let Some(report) = clients
                .iter()
                .filter(|c| c.machine == m.name)
                .find_map(|c| c.report.as_ref())
            {
                m.harnesses = report
                    .harnesses
                    .iter()
                    .filter(|h| h.present)
                    .map(report_line)
                    .collect();
            }
            m.summary = if m.clients.is_empty() {
                "Not paired yet".to_owned()
            } else {
                format!("{} paired", m.clients.len())
            };
        }
    }
    out
}

fn ago(secs: u64) -> String {
    let now = crate::clients::now();
    match now.saturating_sub(secs) {
        s if s < 90 => "just now".to_owned(),
        s if s < 90 * 60 => format!("{} min ago", s / 60),
        s if s < 36 * 3600 => format!("{} h ago", s / 3600),
        s => format!("{} days ago", s / 86_400),
    }
}

fn paired(c: &crate::clients::Client, latest: Option<u64>) -> PairedClient {
    let applied = c.report.as_ref().and_then(|r| r.release);
    let (status, class) = match (applied, latest) {
        (_, None) => ("No update yet".to_owned(), "muted"),
        (Some(a), Some(l)) if a >= l => (format!("Update {a}, up to date"), "ok"),
        (Some(a), Some(l)) => (format!("Update {a}, {} behind", l - a), "warn"),
        (None, Some(_)) => ("Hasn't applied an update".to_owned(), "warn"),
    };
    let seen = c
        .last_seen
        .map_or_else(|| "never seen".to_owned(), |t| format!("seen {}", ago(t)));
    PairedClient {
        id: c.id.clone(),
        user: c.user.clone(),
        status: format!("{status} · {seen}"),
        class,
        problems: c
            .report
            .as_ref()
            .map_or_else(Vec::new, |r| r.problems.clone()),
    }
}

fn report_line(h: &atlas_core::distribution::HarnessReport) -> HarnessLine {
    let mut parts = Vec::new();
    if h.skills_outside > 0 {
        parts.push(format!("{} outside", h.skills_outside));
    }
    if h.broken > 0 {
        parts.push(format!("{} broken", h.broken));
    }
    let (instructions, instructions_class) = match h.instructions.as_str() {
        "linked" => ("Global AGENTS.md linked".to_owned(), "ok"),
        "imports" => ("Global AGENTS.md imported".to_owned(), "ok"),
        "separate" => ("Own instructions file".to_owned(), "warn"),
        _ => ("No global instructions".to_owned(), "warn"),
    };
    HarnessLine {
        icon: icons::icon(h.kind),
        name: short(h.kind),
        skills: format!("{} skills from Atlas", h.skills_from_atlas),
        skills_warn: parts.join(", "),
        instructions,
        instructions_class,
        hooks: match h.hooks_managed {
            0 => String::new(),
            1 => "1 hook".to_owned(),
            n => format!("{n} hooks"),
        },
        dirs: Vec::new(),
    }
}

// ---------------------------------------------------------------- secrets

/// Hides values that look like credentials in a command line before it's shown:
/// `NAME=value` where the name mentions a key, token, secret or password, the word after
/// `Bearer`, and long opaque strings.
#[must_use]
pub fn mask_secrets(command: &str) -> String {
    const HINTS: [&str; 6] = ["KEY", "TOKEN", "SECRET", "PASSWORD", "PASS", "AUTH"];
    let mut out = Vec::new();
    let mut after_bearer = false;
    for word in command.split(' ') {
        let bare = word.trim_matches(['"', '\'']);
        let masked = if after_bearer && !bare.is_empty() {
            Some("••••".to_owned())
        } else if let Some((name, value)) = bare.split_once('=')
            && !value.is_empty()
            && HINTS.iter().any(|h| name.to_uppercase().contains(h))
        {
            Some(format!("{name}=••••"))
        } else if bare.len() >= 24
            && bare
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_+/=.".contains(c))
            && bare.chars().any(|c| c.is_ascii_digit())
            && bare.chars().any(|c| c.is_ascii_alphabetic())
            && !bare.contains('/')
        {
            Some("••••".to_owned())
        } else {
            None
        };
        after_bearer = bare.eq_ignore_ascii_case("bearer");
        out.push(masked.unwrap_or_else(|| word.to_owned()));
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::{join_names, mask_secrets};
    use atlas_core::kernel::HarnessKind;

    #[test]
    fn masks_credentials_but_keeps_ordinary_commands() {
        assert_eq!(
            mask_secrets("python3 ~/.claude/hooks/no-worktrees-in-projects.py"),
            "python3 ~/.claude/hooks/no-worktrees-in-projects.py"
        );
        assert_eq!(mask_secrets("API_KEY=abc123 run.sh"), "API_KEY=•••• run.sh");
        assert_eq!(
            mask_secrets("curl -H \"Authorization: Bearer sk-live-1\" x"),
            "curl -H \"Authorization: Bearer •••• x"
        );
        assert_eq!(
            mask_secrets("notify ghp_abcdefghijklmnop1234567890"),
            "notify ••••"
        );
    }

    #[test]
    fn names_read_as_english() {
        assert_eq!(join_names(&[HarnessKind::Pi]), "Pi");
        assert_eq!(
            join_names(&[HarnessKind::ClaudeCode, HarnessKind::Codex, HarnessKind::Pi]),
            "Claude, Codex and Pi"
        );
    }
}
