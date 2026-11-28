//! `OpenCode`: `"permission": {"skill": {"<name>": "deny"}}` in `opencode.jsonc`.
//!
//! `OpenCode` reads its config as JSONC, so the owner's comments are kept: the file is
//! edited in place ([`super::jsonc`]) and Atlas's entries end with the comment
//! `// atlas:<name>`. The last matching rule decides, so Atlas adds its entries at the end
//! of `permission.skill`. `<name>` is the `name` in the skill's header, which Atlas's
//! skills keep equal to their folder name.
//!
//! A file Atlas creates starts with [`SKELETON`]: a comment saying Atlas made it, and the
//! `$schema` line `OpenCode` would otherwise add itself. When Atlas's entries go and only
//! that is left, the file goes too.

use std::collections::BTreeSet;

use super::jsonc::{self, Member, NewMember, Object, indent_unit};
use super::shape;
use crate::{Change, FormatError};

/// What a file Atlas creates holds before any entry.
pub(super) const SKELETON: &str = "// Atlas made this file to switch skills off for OpenCode, \
and deletes it once nothing else is in it.\n{\n  \"$schema\": \"https://opencode.ai/config.json\"\n}\n";

fn note(name: &str) -> String {
    format!("// atlas:{name}")
}

fn quoted(s: &str) -> String {
    serde_json::Value::from(s).to_string()
}

fn is_ours(text: &str, m: &Member) -> bool {
    m.value.as_str() == Some("deny") && m.note(text) == Some(note(&m.key).as_str())
}

/// The root object, `permission` and `permission.skill`, each if present, checked for
/// shape. Positions are member indexes.
struct Path<'v> {
    root: &'v Object,
    permission: Option<(usize, &'v Object)>,
    skill: Option<(usize, &'v Object)>,
}

fn child<'v>(o: &'v Object, key: &str, at: &str) -> Result<Option<(usize, &'v Object)>, FormatError> {
    o.position(key)
        .map(|i| {
            o.members[i]
                .value
                .as_object()
                .map(|c| (i, c))
                .ok_or_else(|| shape(at, "an object"))
        })
        .transpose()
}

fn locate(root: &jsonc::Value) -> Result<Path<'_>, FormatError> {
    let root = root
        .as_object()
        .ok_or_else(|| shape("the top level", "an object"))?;
    let permission = child(root, "permission", "permission")?;
    let skill = match permission {
        Some((_, p)) => child(p, "skill", "permission.skill")?,
        None => None,
    };
    Ok(Path {
        root,
        permission,
        skill,
    })
}

/// Lines for `names`' entries, each with its note; all but the last end with a comma.
fn entry_lines(names: &[&String], indent: &str) -> String {
    names
        .iter()
        .enumerate()
        .map(|(k, n)| {
            let comma = if k + 1 < names.len() { "," } else { "" };
            format!("{indent}{}: \"deny\"{comma} {}", quoted(n), note(n))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Removes the first of Atlas's entries that shouldn't stay. False when there is none.
fn remove_one(text: &mut String, off: &BTreeSet<String>) -> Result<bool, FormatError> {
    let parsed = jsonc::parse(text)?;
    let path = locate(&parsed)?;
    let Some((_, skill)) = path.skill else {
        return Ok(false);
    };
    let mut seen = BTreeSet::new();
    let stale = skill.members.iter().position(|m| {
        is_ours(text, m) && (!off.contains(&m.key) || !seen.insert(m.key.as_str()))
    });
    match stale {
        Some(i) => {
            *text = jsonc::remove_member(text, skill, i);
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Drops `permission.skill`, then `permission`, when Atlas's removals left them empty.
fn remove_emptied(text: &mut String) -> Result<(), FormatError> {
    let parsed = jsonc::parse(text)?;
    let path = locate(&parsed)?;
    if let (Some((i, skill)), Some((_, permission))) = (path.skill, path.permission) {
        if skill.is_bare(text) {
            *text = jsonc::remove_member(text, permission, i);
        } else {
            return Ok(());
        }
    }
    let parsed = jsonc::parse(text)?;
    let path = locate(&parsed)?;
    if let Some((i, permission)) = path.permission
        && permission.is_bare(text)
    {
        *text = jsonc::remove_member(text, path.root, i);
    }
    Ok(())
}

/// Adds entries for `names` at the end of `permission.skill`, making the objects that
/// aren't there yet.
fn add(text: &str, names: &[&String]) -> Result<String, FormatError> {
    let parsed = jsonc::parse(text)?;
    let path = locate(&parsed)?;
    let unit = indent_unit(text);
    Ok(match (path.permission, path.skill) {
        (_, Some((_, skill))) => {
            let items: Vec<NewMember> = names
                .iter()
                .map(|n| NewMember {
                    body: format!("{}: \"deny\"", quoted(n)),
                    note: Some(note(n)),
                })
                .collect();
            jsonc::append_members(text, skill, &items)
        }
        (Some((_, permission)), None) => {
            let body = format!("\"skill\": {{\n{}\n}}", entry_lines(names, &unit));
            jsonc::append_members(text, permission, &[NewMember { body, note: None }])
        }
        (None, _) => {
            let body = format!(
                "\"permission\": {{\n{unit}\"skill\": {{\n{}\n{unit}}}\n}}",
                entry_lines(names, &unit.repeat(2))
            );
            jsonc::append_members(text, path.root, &[NewMember { body, note: None }])
        }
    })
}

/// Nothing but `{}` and whitespace.
fn is_empty_doc(text: &str) -> Result<bool, FormatError> {
    let parsed = jsonc::parse(text)?;
    Ok(parsed.as_object().is_some_and(|o| {
        o.is_bare(text) && text[..o.open].trim().is_empty() && text[o.close + 1..].trim().is_empty()
    }))
}

pub(super) fn render(current: Option<&[u8]>, off: &BTreeSet<String>) -> Result<Change, FormatError> {
    let original = match current {
        Some(bytes) if !bytes.iter().all(u8::is_ascii_whitespace) => {
            std::str::from_utf8(bytes)?.to_owned()
        }
        _ => SKELETON.to_owned(),
    };
    // A name with `*` or `?` would be a pattern matching other skills too.
    let off: BTreeSet<String> = off
        .iter()
        .filter(|n| !n.contains(['*', '?']))
        .cloned()
        .collect();

    let mut text = original.clone();
    let mut removed = false;
    while remove_one(&mut text, &off)? {
        removed = true;
    }
    if removed {
        remove_emptied(&mut text)?;
    }

    let parsed = jsonc::parse(&text)?;
    let path = locate(&parsed)?;
    let missing: Vec<&String> = off
        .iter()
        .filter(|n| path.skill.is_none_or(|(_, s)| s.get(n).is_none()))
        .collect();
    if !missing.is_empty() {
        text = add(&text, &missing)?;
    }

    Ok(if text == original {
        Change::Unchanged
    } else if text == SKELETON || is_empty_doc(&text)? {
        Change::Remove
    } else {
        Change::Write(text.into_bytes())
    })
}
