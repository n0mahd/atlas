//! The skill library's view of one read of the store: where skills were installed from
//! (the `skills` CLI's locks), which same-named copies differ, and what needs the owner's
//! attention. Worked out once per snapshot, so the list renders without touching disk.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use atlas_core::catalog::{Catalog, Skill, SkillHome};
use atlas_core::distribution::HarnessView;
use atlas_fs::Sources;
use atlas_fs::lock::{self, LockKind};
use atlas_fs::skill_folder::{self, Copy};

use crate::views::{Choice, SkillGroup, SkillRow, SkillSection, Snapshot};

#[derive(Debug, Default)]
pub struct Index {
    /// `owner/repo` of Global skills, by name (the CLI's global lock).
    global_sources: HashMap<String, String>,
    /// `owner/repo` of project skills, by project then name (each `skills-lock.json`).
    project_sources: HashMap<String, HashMap<String, String>>,
    /// Names held in more than one place, with each copy's fingerprint.
    copies: BTreeMap<String, Vec<Copy>>,
}

impl Index {
    /// Reads the locks and fingerprints the copies of names that occur more than once.
    #[must_use]
    pub fn read(
        sources: &Sources,
        global_lock: &Path,
        catalog: &Catalog,
        harnesses: &[HarnessView],
    ) -> Self {
        let read = |path: &Path, kind| {
            lock::read_lock(path, kind)
                .ok()
                .flatten()
                .map_or_else(HashMap::new, |l| {
                    l.records
                        .iter()
                        .filter_map(|(name, r)| Some((name.clone(), r.github()?.slug())))
                        .collect()
                })
        };
        Self {
            global_sources: read(global_lock, LockKind::Global),
            project_sources: sources
                .projects
                .iter()
                .map(|p| {
                    (
                        p.name.to_string(),
                        read(&lock::project_lock_path(&p.root), LockKind::Project),
                    )
                })
                .collect(),
            copies: skill_folder::same_named(sources, catalog, harnesses),
        }
    }

    /// Where the `skills` CLI installed this skill from, as `owner/repo`.
    #[must_use]
    pub fn source(&self, home: &SkillHome, name: &str) -> Option<&str> {
        match home {
            SkillHome::Private | SkillHome::Public => self.global_sources.get(name),
            SkillHome::Project(p) => self.project_sources.get(p.as_str())?.get(name),
        }
        .map(String::as_str)
    }

    /// For the copy at `shown`: the other copies whose `SKILL.md` differs, as words.
    #[must_use]
    pub fn conflict(&self, name: &str, shown: &str) -> Option<String> {
        let copies = self.copies.get(name)?;
        let mine = copies.iter().find(|c| c.shown == shown)?.digest.as_ref()?;
        let others: Vec<&str> = copies
            .iter()
            .filter(|c| c.digest.as_ref().is_some_and(|d| d != mine))
            .map(|c| c.shown.as_str())
            .collect();
        words(&others).map(|w| format!("Conflict with {w}"))
    }

    /// For a name found only outside the store: whether its copies differ.
    #[must_use]
    pub fn outside_conflict(&self, name: &str) -> Option<String> {
        let copies = self.copies.get(name)?;
        let mut digests: Vec<&String> = copies.iter().filter_map(|c| c.digest.as_ref()).collect();
        digests.sort();
        digests.dedup();
        if digests.len() < 2 {
            return None;
        }
        let places: Vec<&str> = copies.iter().map(|c| c.shown.as_str()).collect();
        words(&places).map(|w| format!("Conflict: different copies in {w}"))
    }
}

/// Percent-encodes `s` for one URL query value or path segment.
#[must_use]
pub fn pct(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn words(places: &[&str]) -> Option<String> {
    match places {
        [] => None,
        [one] => Some((*one).to_owned()),
        [one, two] => Some(format!("{one} and {two}")),
        [one, rest @ ..] => Some(format!("{one} and {} more", rest.len())),
    }
}

/// The folder a catalog skill's display path names (without `/SKILL.md`).
fn folder_shown(skill: &Skill) -> &str {
    skill.path.trim_end_matches("/SKILL.md")
}

/// Fills in each row's source, conflict and reasons; keeps only rows that need attention
/// when asked; and moves rows installed from one source into a group per source.
/// Returns how many rows need attention.
pub fn decorate(snap: &Snapshot, sections: &mut [SkillSection], attention: bool) -> usize {
    let by_key: HashMap<(&str, &str), &Skill> = snap
        .catalog
        .skills
        .iter()
        .map(|s| ((s.home.key(), s.name.as_str()), s))
        .collect();
    let mut needing = 0;
    for section in sections.iter_mut() {
        for row in &mut section.rows {
            match by_key.get(&(row.home.as_str(), row.name.as_str())) {
                Some(skill) if !row.home.is_empty() => {
                    reasons_for(snap, skill, row);
                }
                _ => {
                    if row.flag == "Broken link" {
                        row.reasons.push("Broken link".to_owned());
                    }
                    if let Some(c) = snap.library.outside_conflict(&row.name) {
                        row.conflict = true;
                        row.reasons.push(c);
                    }
                }
            }
            if !row.reasons.is_empty() {
                needing += 1;
            }
        }
        if attention {
            section.rows.retain(|r| !r.reasons.is_empty());
        }
        let mut groups: BTreeMap<String, Vec<SkillRow>> = BTreeMap::new();
        let mut plain = Vec::new();
        for row in std::mem::take(&mut section.rows) {
            match row.source.clone() {
                Some(src) => groups.entry(src).or_default().push(row),
                None => plain.push(row),
            }
        }
        section.rows = plain;
        section.groups = groups
            .into_iter()
            .map(|(source, rows)| SkillGroup {
                id: format!(
                    "{}-from-{}",
                    section.id,
                    source
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() {
                            c.to_ascii_lowercase()
                        } else {
                            '-'
                        })
                        .collect::<String>()
                ),
                source,
                rows,
            })
            .collect();
    }
    needing
}

fn reasons_for(snap: &Snapshot, skill: &Skill, row: &mut SkillRow) {
    row.source = snap
        .library
        .source(&skill.home, skill.name.as_str())
        .map(str::to_owned);
    let shown = folder_shown(skill);
    if let SkillHome::Project(_) = skill.home {
        let missing: Vec<&str> = row
            .agents
            .iter()
            .filter(|a| a.class == "warn")
            .map(|a| a.name)
            .collect();
        if missing.contains(&"Claude") {
            row.reasons
                .push("Claude Code can't use it in this project".to_owned());
        }
        let others: Vec<&str> = missing.into_iter().filter(|n| *n != "Claude").collect();
        if !others.is_empty() {
            row.reasons.push(format!("Not in {}", others.join(", ")));
        }
    } else if !row.flag.is_empty() {
        row.reasons.push(row.flag.clone());
    }
    if let Some(c) = snap.library.conflict(skill.name.as_str(), shown) {
        row.conflict = true;
        row.reasons.push(c);
    }
    if let Some(d) = &skill.declared_name {
        row.reasons
            .push(format!("Its SKILL.md names it “{d}”, not {}", skill.name));
    }
    if skill.description.is_empty() {
        row.reasons
            .push("No description, so agents can't tell when to use it".to_owned());
    }
    let prefix = format!("{shown}/");
    for p in &snap.catalog.problems {
        if p.path == shown || p.path.starts_with(&prefix) {
            row.reasons.push(p.message.clone());
        }
    }
}

/// Where a bulk move can send skills: Global and each project.
#[must_use]
pub fn destinations(snap: &Snapshot) -> Vec<Choice> {
    let mut out = vec![Choice {
        value: "private".to_owned(),
        label: "Global".to_owned(),
        checked: false,
        disabled: false,
    }];
    out.extend(snap.projects.iter().map(|p| Choice {
        value: p.project.to_string(),
        label: p.project.to_string(),
        checked: false,
        disabled: false,
    }));
    out
}
