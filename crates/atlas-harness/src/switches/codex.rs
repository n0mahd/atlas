//! Codex: `[[skills.config]]` tables in `~/.codex/config.toml`, each naming a skill by
//! `path` (its `SKILL.md`) or by `name`, with `enabled = false`.
//!
//! Atlas's tables end their header line with the comment `# atlas:<name>`; Codex's own
//! writes to the file keep it (checked on 0.172.0). Codex resolves links in a `path` when
//! it loads, so the path through an agent's skill folder works. Edited with `toml_edit`,
//! so the owner's comments and layout stay as they are.

use std::collections::BTreeSet;

use atlas_core::kernel::Slug;
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use super::SwitchOff;
use crate::change::shape;
use crate::placement::Reach;
use crate::{Change, FormatError};

/// What follows `]]` on the header of a table Atlas wrote.
const MARKER: &str = "# atlas:";

/// How a table names its skill. Atlas's tables are told apart by it: two skills of one
/// name (two projects', or a global one and a project's) each get their own.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Selector {
    Path(String),
    Name(String),
}

/// How Atlas names `skill`: by its `SKILL.md` when that is known, else by name (home
/// only, since a name entry switches off every skill of that name). `None` when it can't
/// be named this time.
fn selector(skill: &SwitchOff, reach: Reach) -> Option<Selector> {
    match (&skill.path, reach) {
        // A path Codex can't be given as TOML text is skipped, not widened to a name.
        (Some(p), _) => p.to_str().map(|p| Selector::Path(p.to_owned())),
        (None, Reach::Home) => Some(Selector::Name(skill.name.clone())),
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

/// The skill an Atlas table switches off and how it names it, if `t` is one.
fn ours(t: &Table) -> Option<(String, Selector)> {
    let suffix = t.decor().suffix()?.as_str()?.trim();
    let name = suffix.strip_prefix(MARKER)?;
    let off = t.get("enabled").and_then(Item::as_bool) == Some(false);
    if !off || Slug::new(name).is_err() {
        return None;
    }
    Some((name.to_owned(), selector_of(t)?))
}

/// Whether an owner's table already decides about the skill Atlas would name with `sel`:
/// a name entry covers every skill of that name; a path entry covers the skill at that
/// exact path, or, when Atlas names the skill by name, any `SKILL.md` in a folder of
/// that name.
fn covers(t: &Table, name: &str, sel: &Selector) -> bool {
    if t.get("name").and_then(Item::as_str) == Some(name) {
        return true;
    }
    let Some(path) = t.get("path").and_then(Item::as_str) else {
        return false;
    };
    match sel {
        Selector::Path(p) => path == p,
        Selector::Name(_) => {
            let file = format!("{name}/SKILL.md");
            path == file || path.ends_with(&format!("/{file}"))
        }
    }
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
    off: &BTreeSet<&SwitchOff>,
    reach: Reach,
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

    // What Atlas's tables should be, and the names it can't name this time (a project's
    // skill without a path), whose tables are kept as they are rather than guessed at.
    let mut wanted = BTreeSet::new();
    let mut held = BTreeSet::new();
    for skill in off {
        match selector(skill, reach) {
            Some(sel) => wanted.insert((skill.name.clone(), sel)),
            None => held.insert(skill.name.clone()),
        };
    }

    // Each Atlas table is kept once, or is stale.
    let mut present = BTreeSet::new();
    let mut stale: Vec<bool> = config
        .iter()
        .map(|t| {
            ours(t).is_some_and(|key| {
                let keep = wanted.contains(&key) || held.contains(&key.0);
                !(keep && present.insert(key))
            })
        })
        .collect();

    let mut changed = false;
    let mut added = Vec::new();
    for key @ (name, sel) in &wanted {
        let owners = config
            .iter()
            .any(|t| ours(t).is_none() && covers(t, name, sel));
        if present.contains(key) || owners {
            continue;
        }
        // A stale table of the same name is reused in place (the skill's folder moved),
        // keeping its spot and comments.
        let reuse = config
            .iter()
            .zip(&stale)
            .position(|(t, s)| *s && ours(t).is_some_and(|(n, _)| n == *name));
        match reuse.and_then(|i| config.get_mut(i).map(|t| (i, t))) {
            Some((i, t)) => {
                let mut fresh = table(name, sel);
                *fresh.decor_mut() = t.decor().clone();
                fresh.set_position(t.position());
                *t = fresh;
                stale[i] = false;
            }
            None => added.push(table(name, sel)),
        }
        changed = true;
    }
    let before = config.len();
    let mut at = 0;
    config.retain(|_| {
        at += 1;
        !stale[at - 1]
    });
    changed |= config.len() != before;
    for t in added {
        config.push(t);
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
