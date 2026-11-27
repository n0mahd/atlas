//! View models: what each page shows, built from one snapshot of the store.

use std::collections::{BTreeSet, HashSet};

use atlas_core::catalog::{Catalog, MemoryKind, Skill, SkillHome};
use atlas_core::distribution::{HarnessView, InstalledSkill, InstructionLink};
use atlas_core::kernel::{HarnessKind, Scope};

use crate::markdown;
use crate::roman::roman;

/// Everything read for one request.
#[derive(Debug)]
pub struct Snapshot {
    pub catalog: Catalog,
    pub harnesses: Vec<HarnessView>,
}

/// The harnesses that get a column on the Skills page. T3 Code runs the others.
pub const COLUMNS: [HarnessKind; 5] = [
    HarnessKind::ClaudeCode,
    HarnessKind::Codex,
    HarnessKind::OpenCode,
    HarnessKind::Pi,
    HarnessKind::Gemini,
];

const fn letter(kind: HarnessKind) -> &'static str {
    match kind {
        HarnessKind::ClaudeCode => "C",
        HarnessKind::Codex => "X",
        HarnessKind::OpenCode => "O",
        HarnessKind::Pi => "P",
        HarnessKind::Gemini => "G",
        HarnessKind::T3 => "T",
    }
}

// ---------------------------------------------------------------- page chrome

#[derive(Debug, Clone)]
pub struct NavItem {
    pub num: &'static str,
    pub label: &'static str,
    pub href: &'static str,
    pub active: bool,
    pub later: bool,
}

/// What every page's frame shows.
#[derive(Debug, Clone)]
pub struct Page {
    pub title: String,
    pub folio: String,
    pub hub: String,
    pub nav: Vec<NavItem>,
    pub problems: usize,
}

impl Page {
    #[must_use]
    pub fn new(section: &str, title: impl Into<String>, hub: &str, snap: &Snapshot) -> Self {
        let items = [
            ("I", "Skills", "/skills", false),
            ("II", "Instructions", "/instructions", false),
            ("III", "Memory", "/memory", false),
            ("IV", "Hooks", "/hooks", false),
            ("V", "MCP servers", "/mcp", true),
            ("VI", "Machines", "/machines", false),
        ];
        let nav: Vec<NavItem> = items
            .into_iter()
            .map(|(num, label, href, later)| NavItem {
                num,
                label,
                href,
                active: href == section,
                later,
            })
            .collect();
        let folio = nav.iter().find(|n| n.active).map_or_else(
            || "Frontispiece".to_owned(),
            |n| format!("Folio {} · {}", n.num, n.label),
        );
        Self {
            title: title.into(),
            folio,
            hub: hub.to_owned(),
            nav,
            problems: snap.catalog.problems.len(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Choice {
    pub value: String,
    pub label: String,
    pub checked: bool,
}

fn choices(options: &[(&str, &str)], current: &str) -> Vec<Choice> {
    options
        .iter()
        .map(|(value, label)| Choice {
            value: (*value).to_owned(),
            label: (*label).to_owned(),
            checked: *value == current,
        })
        .collect()
}

fn matches(q: &str, fields: &[&str]) -> bool {
    let q = q.trim().to_lowercase();
    q.is_empty() || fields.iter().any(|f| f.to_lowercase().contains(&q))
}

fn scope_label(scope: &Scope) -> String {
    match scope {
        Scope::Global => "Global".to_owned(),
        Scope::Project(p) => format!("Project · {p}"),
    }
}

fn home_label(home: &SkillHome) -> String {
    match home {
        SkillHome::Private => "Global · private".to_owned(),
        SkillHome::Public => "Global · public".to_owned(),
        SkillHome::Project(p) => format!("Project · {p}"),
    }
}

fn memory_names(catalog: &Catalog) -> HashSet<&str> {
    catalog.memory.iter().map(|m| m.name.as_str()).collect()
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
    pub outside: usize,
    pub harnesses_present: usize,
}

#[must_use]
pub fn overview(snap: &Snapshot) -> Overview {
    let c = &snap.catalog;
    let projects: BTreeSet<_> = c
        .skills
        .iter()
        .filter_map(|s| s.home.scope().project().cloned())
        .collect();
    let global = c
        .skills
        .iter()
        .filter(|s| s.home.scope() == Scope::Global)
        .count();
    let hosts: BTreeSet<_> = c.memory.iter().map(|m| m.host.as_str()).collect();
    let found_hooks: usize = snap.harnesses.iter().map(|h| h.hooks.len()).sum();
    Overview {
        tiles: vec![
            Tile {
                num: "I",
                label: "Skills",
                count: c.skills.len(),
                detail: format!(
                    "{global} global · {} in {} projects",
                    c.skills.len() - global,
                    projects.len()
                ),
                href: "/skills",
            },
            Tile {
                num: "II",
                label: "Instructions",
                count: c.instructions.len(),
                detail: "AGENTS.md and CLAUDE.md files".to_owned(),
                href: "/instructions",
            },
            Tile {
                num: "III",
                label: "Memory",
                count: c.memory.len(),
                detail: format!("pages across {} hosts", hosts.len()),
                href: "/memory",
            },
            Tile {
                num: "IV",
                label: "Hooks",
                count: c.hooks.len(),
                detail: format!("in Atlas · {found_hooks} found in harness configs"),
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
        outside: outside_skills(snap).len(),
        harnesses_present: snap
            .harnesses
            .iter()
            .filter(|h| h.present && h.kind != HarnessKind::T3)
            .count(),
    }
}

// ---------------------------------------------------------------- skills

#[derive(Debug, Clone)]
pub struct Mark {
    pub letter: &'static str,
    pub label: String,
    /// `on`, `off` or `na`.
    pub class: &'static str,
}

#[derive(Debug, Clone)]
pub struct SkillRow {
    pub plate: String,
    pub name: String,
    pub description: String,
    pub href: Option<String>,
    pub scope_label: String,
    /// `global`, `project` or `outside`.
    pub scope_class: &'static str,
    pub marks: Vec<Mark>,
    pub status: String,
    /// `ok`, `muted` or `warn`.
    pub status_class: &'static str,
}

#[derive(Debug)]
pub struct SkillsList {
    pub rows: Vec<SkillRow>,
    pub total: usize,
    pub q: String,
    pub filters: Vec<Choice>,
    pub columns: Vec<(&'static str, &'static str)>,
    pub outside: usize,
}

fn view(snap: &Snapshot, kind: HarnessKind) -> Option<&HarnessView> {
    snap.harnesses.iter().find(|h| h.kind == kind)
}

fn atlas_marks(snap: &Snapshot, skill: &Skill) -> (Vec<Mark>, String, &'static str) {
    if let SkillHome::Project(p) = &skill.home {
        let marks = COLUMNS
            .iter()
            .map(|&k| Mark {
                letter: letter(k),
                label: format!("{} reads it inside the {p} repo", k.display_name()),
                class: "na",
            })
            .collect();
        return (marks, "read in the repo".to_owned(), "muted");
    }
    let mut missing = Vec::new();
    let mut installed = 0;
    let marks = COLUMNS
        .iter()
        .map(|&k| {
            let v = view(snap, k);
            let present = v.is_some_and(|v| v.present);
            let on = v.is_some_and(|v| v.sees(skill.name.as_str()));
            if present && on {
                installed += 1;
            } else if present {
                missing.push(k.display_name());
            }
            let label = match (present, on) {
                (false, _) => format!("{} isn't set up on this hub", k.display_name()),
                (true, true) => format!("{} has {}", k.display_name(), skill.name),
                (true, false) => format!("{} doesn't have {}", k.display_name(), skill.name),
            };
            Mark {
                letter: letter(k),
                label,
                class: if on {
                    "on"
                } else if present {
                    "off"
                } else {
                    "na"
                },
            }
        })
        .collect();
    let (status, class) = if missing.is_empty() {
        (format!("✓ all {installed}"), "ok")
    } else {
        (format!("△ not in {}", missing.join(", ")), "warn")
    };
    (marks, status, class)
}

/// Skills a harness on this hub has that don't come from the store.
fn outside_skills(snap: &Snapshot) -> Vec<(String, Vec<HarnessKind>, bool)> {
    let known: HashSet<&str> = snap
        .catalog
        .skills
        .iter()
        .map(|s| s.name.as_str())
        .collect();
    let mut found: Vec<(String, Vec<HarnessKind>, bool)> = Vec::new();
    for h in &snap.harnesses {
        for entry in h.skill_dirs.iter().flat_map(|d| &d.entries) {
            let broken = matches!(entry, InstalledSkill::Broken { .. });
            let foreign = matches!(entry, InstalledSkill::Outside { .. }) || broken;
            if !foreign || known.contains(entry.name()) {
                continue;
            }
            match found.iter_mut().find(|(n, ..)| n == entry.name()) {
                Some((_, kinds, b)) => {
                    if !kinds.contains(&h.kind) {
                        kinds.push(h.kind);
                    }
                    *b |= broken;
                }
                None => found.push((entry.name().to_owned(), vec![h.kind], broken)),
            }
        }
    }
    found.sort();
    found
}

#[must_use]
pub fn skills(snap: &Snapshot, q: &str, filter: &str) -> SkillsList {
    let mut rows: Vec<SkillRow> = snap
        .catalog
        .skills
        .iter()
        .map(|s| {
            let (marks, status, status_class) = atlas_marks(snap, s);
            SkillRow {
                plate: String::new(),
                name: s.name.to_string(),
                description: s.description.clone(),
                href: Some(format!("/skills/{}/{}", s.home.key(), s.name)),
                scope_label: home_label(&s.home),
                scope_class: if matches!(s.home, SkillHome::Project(_)) {
                    "project"
                } else {
                    "global"
                },
                marks,
                status,
                status_class,
            }
        })
        .collect();
    let outside = outside_skills(snap);
    let outside_count = outside.len();
    rows.extend(outside.into_iter().map(|(name, kinds, broken)| {
        SkillRow {
            plate: String::new(),
            marks: COLUMNS
                .iter()
                .map(|&k| Mark {
                    letter: letter(k),
                    label: format!(
                        "{} {} {name}",
                        k.display_name(),
                        if kinds.contains(&k) {
                            "has"
                        } else {
                            "doesn't have"
                        }
                    ),
                    class: if kinds.contains(&k) { "on" } else { "off" },
                })
                .collect(),
            description: if broken {
                "A link in a harness folder whose target is gone.".to_owned()
            } else {
                "Sits in a harness's own folder on this hub, not in the store.".to_owned()
            },
            name,
            href: None,
            scope_label: "Outside Atlas".to_owned(),
            scope_class: "outside",
            status: if broken {
                "broken link".to_owned()
            } else {
                "this hub only".to_owned()
            },
            status_class: "warn",
        }
    }));
    let total = rows.len();
    rows.retain(|r| {
        let by_filter = match filter {
            "global" => r.scope_class == "global",
            "project" => r.scope_class == "project",
            "outside" => r.scope_class == "outside",
            _ => true,
        };
        by_filter && matches(q, &[&r.name, &r.description, &r.scope_label])
    });
    for (i, row) in rows.iter_mut().enumerate() {
        row.plate = roman(i + 1);
    }
    SkillsList {
        rows,
        total,
        q: q.to_owned(),
        filters: choices(
            &[
                ("all", "All"),
                ("global", "Global"),
                ("project", "Project"),
                ("outside", "Outside Atlas"),
            ],
            if filter.is_empty() { "all" } else { filter },
        ),
        columns: COLUMNS
            .iter()
            .map(|&k| (letter(k), k.display_name()))
            .collect(),
        outside: outside_count,
    }
}

#[derive(Debug)]
pub struct SeenBy {
    pub harness: &'static str,
    pub detail: String,
    pub class: &'static str,
}

#[derive(Debug)]
pub struct SkillDetail {
    pub name: String,
    pub description: String,
    pub declared_name: Option<String>,
    pub scope_label: String,
    pub path: String,
    pub files: Vec<String>,
    pub seen_by: Vec<SeenBy>,
    pub body_html: String,
}

#[must_use]
pub fn skill_detail(snap: &Snapshot, home: &str, name: &str) -> Option<SkillDetail> {
    let skill = snap
        .catalog
        .skills
        .iter()
        .find(|s| s.home.key() == home && s.name.as_str() == name)?;
    let seen_by = COLUMNS
        .iter()
        .map(|&k| {
            let v = view(snap, k);
            let (detail, class) = match (&skill.home, v) {
                (SkillHome::Project(p), _) => (format!("reads it inside the {p} repo"), "muted"),
                (_, Some(v)) if !v.present => ("not set up on this hub".to_owned(), "muted"),
                (_, Some(v)) => v
                    .skill_dirs
                    .iter()
                    .find(|d| d.entries.iter().any(|e| e.name() == name))
                    .map_or(("doesn't have it".to_owned(), "warn"), |d| {
                        (format!("linked in {}", d.path), "ok")
                    }),
                (_, None) => ("unknown".to_owned(), "muted"),
            };
            SeenBy {
                harness: k.display_name(),
                detail,
                class,
            }
        })
        .collect();
    Some(SkillDetail {
        name: skill.name.to_string(),
        description: skill.description.clone(),
        declared_name: skill.declared_name.clone(),
        scope_label: home_label(&skill.home),
        path: skill.path.clone(),
        files: skill.files.clone(),
        seen_by,
        body_html: markdown::render(&skill.body, &memory_names(&snap.catalog)),
    })
}

// ---------------------------------------------------------------- instructions

#[derive(Debug)]
pub struct InstructionRow {
    pub plate: String,
    pub title: String,
    pub scope_label: String,
    pub scope_class: &'static str,
    pub path: String,
    pub href: String,
    pub lines: usize,
    pub readers: String,
}

#[derive(Debug)]
pub struct Reader {
    pub harness: &'static str,
    pub path: String,
    pub state: &'static str,
    pub class: &'static str,
}

fn instruction_key(scope: &Scope, file: &str) -> String {
    match scope {
        Scope::Global => format!("/instructions/global/{file}"),
        Scope::Project(p) => format!("/instructions/{p}/{file}"),
    }
}

fn global_readers(snap: &Snapshot) -> Vec<Reader> {
    snap.harnesses
        .iter()
        .filter(|h| h.kind != HarnessKind::T3)
        .flat_map(|h| {
            h.instructions.iter().map(move |i| {
                let (state, class) = match i.link {
                    InstructionLink::Linked => ("linked to the store", "ok"),
                    InstructionLink::Imports => ("imports the store's file", "ok"),
                    InstructionLink::Separate => ("its own file, not from Atlas", "warn"),
                    InstructionLink::Missing if h.present => ("missing", "warn"),
                    InstructionLink::Missing => ("harness not set up", "muted"),
                };
                Reader {
                    harness: h.kind.display_name(),
                    path: i.path.clone(),
                    state,
                    class,
                }
            })
        })
        .collect()
}

#[must_use]
pub fn instructions(snap: &Snapshot) -> Vec<InstructionRow> {
    let readers = global_readers(snap);
    let reading: Vec<&str> = readers
        .iter()
        .filter(|r| r.class == "ok")
        .map(|r| r.harness)
        .collect();
    snap.catalog
        .instructions
        .iter()
        .enumerate()
        .map(|(i, f)| InstructionRow {
            plate: roman(i + 1),
            title: match &f.scope {
                Scope::Global => format!("Global {}", f.file_name),
                Scope::Project(p) => format!("{p} · {}", f.file_name),
            },
            scope_label: scope_label(&f.scope),
            scope_class: if f.scope == Scope::Global {
                "global"
            } else {
                "project"
            },
            path: f.path.clone(),
            href: instruction_key(&f.scope, &f.file_name),
            lines: f.body.raw().as_str().lines().count(),
            readers: match f.scope {
                Scope::Global if reading.is_empty() => {
                    "No harness on this hub reads it yet".to_owned()
                }
                Scope::Global => format!("Read by {}", reading.join(", ")),
                Scope::Project(_) => "Read by agents working in the repo".to_owned(),
            },
        })
        .collect()
}

#[derive(Debug)]
pub struct InstructionDetail {
    pub title: String,
    pub scope_label: String,
    pub path: String,
    pub readers: Vec<Reader>,
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
    Some(InstructionDetail {
        title: match &f.scope {
            Scope::Global => format!("Global {}", f.file_name),
            Scope::Project(p) => format!("{p} · {}", f.file_name),
        },
        scope_label: scope_label(&f.scope),
        path: f.path.clone(),
        readers: if f.scope == Scope::Global {
            global_readers(snap)
        } else {
            Vec::new()
        },
        body_html: markdown::render(&f.body, &memory_names(&snap.catalog)),
    })
}

// ---------------------------------------------------------------- memory

#[derive(Debug)]
pub struct MemoryRow {
    pub plate: String,
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
}

const fn kind_class(kind: &MemoryKind) -> &'static str {
    match kind {
        MemoryKind::Unknown(_) => "warn",
        _ => "plain",
    }
}

#[must_use]
pub fn memory(snap: &Snapshot, q: &str, kind: &str, host: &str) -> MemoryList {
    let pages = &snap.catalog.memory;
    let mut hosts: Vec<(String, String)> = vec![("all".into(), "Every host".into())];
    let names: BTreeSet<&str> = pages
        .iter()
        .map(|m| m.host.as_str())
        .filter(|h| *h != "all")
        .collect();
    hosts.push(("shared".into(), "Shared (all)".into()));
    hosts.extend(names.into_iter().map(|h| (h.to_owned(), h.to_owned())));
    let host_opts: Vec<(&str, &str)> = hosts
        .iter()
        .map(|(v, l)| (v.as_str(), l.as_str()))
        .collect();
    let kind_now = if kind.is_empty() { "all" } else { kind };
    let host_now = if host.is_empty() { "all" } else { host };
    let rows: Vec<MemoryRow> = pages
        .iter()
        .filter(|m| kind_now == "all" || m.kind.label() == kind_now)
        .filter(|m| match host_now {
            "all" => true,
            "shared" => m.host == "all",
            h => m.host == h,
        })
        .filter(|m| matches(q, &[&m.name, &m.description]))
        .enumerate()
        .map(|(i, m)| MemoryRow {
            plate: roman(i + 1),
            name: m.name.clone(),
            description: m.description.clone(),
            kind: m.kind.label().to_owned(),
            kind_class: kind_class(&m.kind),
            host: m.host.clone(),
            href: format!("/memory/{}", m.name),
        })
        .collect();
    MemoryList {
        rows,
        total: pages.len(),
        q: q.to_owned(),
        kinds: choices(
            &[
                ("all", "All"),
                ("user", "User"),
                ("feedback", "Feedback"),
                ("project", "Project"),
                ("reference", "Reference"),
                ("untyped", "Untyped"),
            ],
            kind_now,
        ),
        hosts: choices(&host_opts, host_now),
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
pub fn memory_detail(snap: &Snapshot, name: &str) -> Option<MemoryDetail> {
    let pages = &snap.catalog.memory;
    let page = pages.iter().find(|m| m.name == name)?;
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
    pub plate: String,
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
    pub plate: String,
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
        .enumerate()
        .map(|(i, h)| AtlasHookRow {
            plate: roman(i + 1),
            name: h.name.to_string(),
            scope_label: scope_label(&h.scope),
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
                || "every harness with this event".to_owned(),
                |hs| {
                    hs.iter()
                        .map(|k| k.display_name())
                        .collect::<Vec<_>>()
                        .join(", ")
                },
            ),
            path: h.path.clone(),
            ok: h.script_present,
        })
        .collect();
    let found = snap
        .harnesses
        .iter()
        .flat_map(|h| {
            h.hooks.iter().map(move |f| {
                (
                    h.kind.display_name(),
                    h.hook_config.clone().unwrap_or_default(),
                    f,
                )
            })
        })
        .enumerate()
        .map(|(i, (harness, config, f))| FoundHookRow {
            plate: roman(i + 1),
            harness,
            event: f.event.clone(),
            matcher: f.matcher.clone().unwrap_or_else(|| "every tool".to_owned()),
            command: f.command.clone(),
            timeout: f
                .timeout
                .map_or_else(|| "default".to_owned(), |t| format!("{t}s")),
            config,
        })
        .collect();
    Hooks { atlas, found }
}

// ---------------------------------------------------------------- machines

#[derive(Debug)]
pub struct DirRow {
    pub path: String,
    pub exists: bool,
    pub from_atlas: usize,
    pub outside: usize,
    pub broken: usize,
}

#[derive(Debug)]
pub struct HarnessCard {
    pub plate: String,
    pub name: &'static str,
    pub present: bool,
    pub runs_others: bool,
    pub dirs: Vec<DirRow>,
    pub readers: Vec<Reader>,
    pub hook_config: Option<String>,
    pub hooks: usize,
}

#[must_use]
pub fn machines(snap: &Snapshot) -> Vec<HarnessCard> {
    let readers = global_readers(snap);
    snap.harnesses
        .iter()
        .enumerate()
        .map(|(i, h)| HarnessCard {
            plate: roman(i + 1),
            name: h.kind.display_name(),
            present: h.present,
            runs_others: h.kind == HarnessKind::T3,
            dirs: h
                .skill_dirs
                .iter()
                .map(|d| DirRow {
                    path: d.path.clone(),
                    exists: d.exists,
                    from_atlas: d
                        .entries
                        .iter()
                        .filter(|e| matches!(e, InstalledSkill::FromAtlas { .. }))
                        .count(),
                    outside: d
                        .entries
                        .iter()
                        .filter(|e| matches!(e, InstalledSkill::Outside { .. }))
                        .count(),
                    broken: d
                        .entries
                        .iter()
                        .filter(|e| matches!(e, InstalledSkill::Broken { .. }))
                        .count(),
                })
                .collect(),
            readers: readers
                .iter()
                .filter(|r| r.harness == h.kind.display_name())
                .map(|r| Reader {
                    harness: r.harness,
                    path: r.path.clone(),
                    state: r.state,
                    class: r.class,
                })
                .collect(),
            hook_config: h.hook_config.clone(),
            hooks: h.hooks.len(),
        })
        .collect()
}
