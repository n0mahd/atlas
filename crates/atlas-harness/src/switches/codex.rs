//! Codex: `[[skills.config]]` tables in `~/.codex/config.toml`, each naming a skill by
//! `path` (its `SKILL.md`) or by `name`, with `enabled = false`.
//!
//! Atlas's tables end their header line with the comment `# atlas:<name>`; Codex's own
//! writes to the file keep it (checked on 0.172.0). Codex resolves links in a `path` when
//! it loads, so the path through an agent's skill folder works. Edited with `toml_edit`,
//! so the owner's comments and layout stay as they are.

use std::collections::BTreeSet;
use std::path::PathBuf;

use atlas_core::kernel::Slug;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::change::shape;
use crate::placement::Reach;
use crate::{Change, FormatError};

/// What follows `]]` on the header of a table Atlas wrote.
const MARKER: &str = "# atlas:";

#[derive(PartialEq, Eq)]
enum Selector {
    Path(String),
    Name(String),
}

/// How Atlas names the skill: by its `SKILL.md` when the folder is known, else by name
/// (home only, since a name entry switches off every skill of that name).
fn selector(
    name: &str,
    reach: Reach,
    skill_dir: &dyn Fn(&str) -> Option<PathBuf>,
) -> Option<Selector> {
    let path = skill_dir(name).and_then(|dir| dir.join("SKILL.md").to_str().map(str::to_owned));
    match (path, reach) {
        (Some(p), _) => Some(Selector::Path(p)),
        (None, Reach::Home) => Some(Selector::Name(name.to_owned())),
        (None, Reach::Project) => None,
    }
}

fn selector_of(t: &Table) -> Option<Selector> {
    match (
        t.get("path").and_then(Item::as_str),
        t.get("name").and_then(Item::as_str),
    ) {
        (Some(p), None) => Some(Selector::Path(p.to_owned())),
        (None, Some(n)) => Some(Selector::Name(n.to_owned())),
        _ => None,
    }
}

/// The skill an Atlas table switches off, if `t` is one.
fn ours(t: &Table) -> Option<String> {
    let suffix = t.decor().suffix()?.as_str()?.trim();
    let name = suffix.strip_prefix(MARKER)?;
    let off = t.get("enabled").and_then(Item::as_bool) == Some(false);
    (off && Slug::new(name).is_ok() && selector_of(t).is_some()).then(|| name.to_owned())
}

/// Whether an owner's table names the skill: by name, or by a path to a `SKILL.md` in a
/// folder of that name.
fn names_skill(t: &Table, name: &str) -> bool {
    let file = format!("{name}/SKILL.md");
    t.get("name").and_then(Item::as_str) == Some(name)
        || t.get("path")
            .and_then(Item::as_str)
            .is_some_and(|p| p == file || p.ends_with(&format!("/{file}")))
}

fn table(name: &str, sel: &Selector) -> Table {
    let mut t = Table::new();
    match sel {
        Selector::Path(p) => t.insert("path", value(p.as_str())),
        Selector::Name(n) => t.insert("name", value(n.as_str())),
    };
    t.insert("enabled", value(false));
    t.decor_mut().set_suffix(format!(" {MARKER}{name}"));
    t
}

pub(super) fn render(
    current: Option<&[u8]>,
    off: &BTreeSet<String>,
    reach: Reach,
    skill_dir: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<Change, FormatError> {
    let text = match current {
        Some(bytes) => std::str::from_utf8(bytes)?,
        None => "",
    };
    let mut doc: DocumentMut = text.parse()?;
    let skills = match doc.get("skills") {
        None => None,
        Some(Item::Table(t)) => Some(t),
        Some(_) => return Err(shape("skills", "a [skills] table")),
    };
    let mut config = match skills.and_then(|s| s.get("config")) {
        None => ArrayOfTables::new(),
        Some(Item::ArrayOfTables(a)) => a.clone(),
        Some(_) => return Err(shape("skills.config", "[[skills.config]] tables")),
    };

    let mut changed = false;
    let mut kept = BTreeSet::new();
    for t in config.iter_mut() {
        let Some(name) = ours(t) else { continue };
        if !off.contains(&name) || kept.contains(&name) {
            continue;
        }
        let Some(want) = selector(&name, reach, skill_dir) else {
            // No folder known this time: keep the table as it is.
            kept.insert(name);
            continue;
        };
        if selector_of(t).as_ref() != Some(&want) {
            let mut fresh = table(&name, &want);
            *fresh.decor_mut() = t.decor().clone();
            fresh.set_position(t.position());
            *t = fresh;
            changed = true;
        }
        kept.insert(name);
    }
    let mut seen = BTreeSet::new();
    let before = config.len();
    config.retain(|t| match ours(t) {
        Some(name) => kept.contains(&name) && seen.insert(name),
        None => true,
    });
    changed |= config.len() != before;
    for name in off {
        let owners = config
            .iter()
            .any(|t| ours(t).is_none() && names_skill(t, name));
        if kept.contains(name) || owners {
            continue;
        }
        if let Some(sel) = selector(name, reach, skill_dir) {
            config.push(table(name, &sel));
            changed = true;
        }
    }
    if !changed {
        return Ok(Change::Unchanged);
    }

    match doc.get_mut("skills").and_then(Item::as_table_mut) {
        Some(skills) if config.is_empty() => {
            skills.remove("config");
            if skills.is_empty() {
                doc.remove("skills");
            }
        }
        Some(skills) => {
            skills.insert("config", Item::ArrayOfTables(config));
        }
        None => {
            let mut skills = Table::new();
            skills.set_implicit(true);
            skills.insert("config", Item::ArrayOfTables(config));
            doc.insert("skills", Item::Table(skills));
        }
    }
    let out = doc.to_string();
    Ok(if out == text {
        Change::Unchanged
    } else if out.trim().is_empty() {
        Change::Remove
    } else {
        Change::Write(out.into_bytes())
    })
}
