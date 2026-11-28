//! What an update changes, for the owner to see before the passkey signs it. Hooks run on
//! every machine by themselves, so a new or changed hook is shown in full before sending;
//! skills, instructions, memory notes and subagents are only counted.
//!
//! Everything here is read from the exact commits the update pins (never HEAD), so the
//! page shows what the signature will cover.

use std::collections::BTreeSet;
use std::path::PathBuf;

use atlas_core::catalog::{HookEvent, HookManifest};
use atlas_core::release::{Manifest, OwnerKey, SourceId};
use atlas_fs::Sources;
use atlas_git::Repo;

/// Largest file shown on the page.
const SHOW_MAX: u64 = 256 * 1024;
/// Past this many commits made outside Atlas, every change counts as "since the last update".
const COMMITS_MAX: usize = 300;

/// What a changed path is to the owner.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Kind {
    /// Runs on the machines without an agent choosing to: needs the owner's look.
    Runs(Runs),
    /// Agents read or act on it: counted only.
    Acted(Acted),
    /// Not something machines use.
    Other,
}

/// Things that run by themselves. An MCP server entry in `mcp.toml` joins here in phase 4.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Runs {
    /// `hooks/<name>/…`, or `hooks/<name>` itself.
    Hook(String),
    /// `hooks` itself (a link there would move every hook).
    HooksFolder,
}

/// A file whose frontmatter may declare hooks the harness runs while it's in use.
#[derive(Clone, Debug, PartialEq, Eq)]
enum WithHooks {
    Skill(String),
    Subagent(String),
}

/// The skill or subagent file at `path`, if its frontmatter can carry hooks.
fn with_hooks(source: &SourceId, path: &str) -> Option<WithHooks> {
    let parts: Vec<&str> = path.split('/').collect();
    match (source, parts.as_slice()) {
        (SourceId::Knowledge, ["skills", name, "SKILL.md"])
        | (SourceId::PublicSkills, [name, "SKILL.md"])
            if !name.starts_with('.') =>
        {
            Some(WithHooks::Skill((*name).to_owned()))
        }
        (SourceId::Knowledge, ["agents", file]) => file
            .strip_suffix(".md")
            .map(|n| WithHooks::Subagent(n.to_owned())),
        _ => None,
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Acted {
    Note(String),
    Skill(String),
    Instructions(String),
    Subagent(String),
}

/// Sorts one path of a store repo.
fn classify(source: &SourceId, path: &str) -> Kind {
    let parts: Vec<&str> = path.split('/').collect();
    match (source, parts.as_slice()) {
        (SourceId::Knowledge, ["hooks"]) => Kind::Runs(Runs::HooksFolder),
        (SourceId::Knowledge, ["hooks", name, ..]) => Kind::Runs(Runs::Hook((*name).to_owned())),
        (SourceId::Knowledge, ["skills", name, _, ..])
        | (SourceId::PublicSkills, [name, _, ..])
            if !name.starts_with('.') =>
        {
            Kind::Acted(Acted::Skill((*name).to_owned()))
        }
        (SourceId::Knowledge, ["AGENTS.md"]) => Kind::Acted(Acted::Instructions(path.to_owned())),
        (SourceId::Knowledge, ["memory", .., last]) if last.strip_suffix(".md").is_some() => {
            Kind::Acted(Acted::Note(path.to_owned()))
        }
        (SourceId::Knowledge, ["agents", file]) if file.strip_suffix(".md").is_some() => {
            Kind::Acted(Acted::Subagent(file.trim_end_matches(".md").to_owned()))
        }
        _ => Kind::Other,
    }
}

/// One line of a diff, with how to show it.
#[derive(Clone, Debug)]
pub(crate) struct DiffLine {
    /// `add`, `del`, `ctx`, `hunk` or `file`.
    pub class: &'static str,
    pub text: String,
}

/// A file of a new hook, shown in full.
#[derive(Clone, Debug)]
pub(crate) struct HookFile {
    /// Its name inside the hook's folder.
    pub name: String,
    pub text: String,
    /// Set instead of text when it can't be shown.
    pub note: String,
}

/// A hook that is new or changed in this update.
#[derive(Clone, Debug)]
pub(crate) struct HookChange {
    pub name: String,
    /// Hooks declared in a skill's or subagent's frontmatter: "in the skill writing".
    pub place: String,
    pub new: bool,
    pub description: String,
    /// When it runs, in plain words.
    pub when: String,
    /// Which tools it applies to, when limited.
    pub matcher: String,
    /// New hooks: the script first, then any other files in its folder.
    pub files: Vec<HookFile>,
    /// Changed hooks: the change to its folder.
    pub diff: Vec<DiffLine>,
    /// Its script isn't marked executable, so machines skip the hook.
    pub not_runnable: bool,
    /// Set when its folder, its script or another file in it is a link: machines skip
    /// such a hook, and this says why.
    pub link: String,
}

/// Counts of the other things agents act on that changed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Others {
    items: BTreeSet<Acted>,
    /// True: only changes made outside Atlas's own editor are counted.
    outside: bool,
}

impl Others {
    /// "3 notes and 1 skill changed outside Atlas", or None when nothing changed.
    pub(crate) fn line(&self) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        let count = |f: fn(&Acted) -> bool| self.items.iter().filter(|a| f(a)).count();
        let parts: Vec<String> = [
            (count(|a| matches!(a, Acted::Note(_))), "note", "notes"),
            (count(|a| matches!(a, Acted::Skill(_))), "skill", "skills"),
            (
                count(|a| matches!(a, Acted::Instructions(_))),
                "instruction file",
                "instruction files",
            ),
            (
                count(|a| matches!(a, Acted::Subagent(_))),
                "subagent file",
                "subagent files",
            ),
        ]
        .into_iter()
        .filter(|(n, _, _)| *n > 0)
        .map(|(n, one, many)| format!("{n} {}", if n == 1 { one } else { many }))
        .collect();
        let list = match parts.as_slice() {
            [] => return None,
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        let when = if self.outside {
            "outside Atlas"
        } else {
            "since the last update"
        };
        Some(format!("{list} changed {when}"))
    }

    /// "1 note changed", "AGENTS.md and 2 skills changed": what an update carries, for the
    /// line beside the passkey tap. None when nothing counted changed.
    pub(crate) fn summary(&self) -> Option<String> {
        let count = |f: fn(&Acted) -> bool| self.items.iter().filter(|a| f(a)).count();
        let plural =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let mut parts: Vec<String> = self
            .items
            .iter()
            .filter_map(|a| match a {
                Acted::Instructions(p) => Some(p.clone()),
                _ => None,
            })
            .collect();
        for (n, one, many) in [
            (count(|a| matches!(a, Acted::Note(_))), "note", "notes"),
            (count(|a| matches!(a, Acted::Skill(_))), "skill", "skills"),
            (
                count(|a| matches!(a, Acted::Subagent(_))),
                "subagent file",
                "subagent files",
            ),
        ] {
            if n > 0 {
                parts.push(plural(n, one, many));
            }
        }
        let list = match parts.as_slice() {
            [] => return None,
            [one] => one.clone(),
            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
        };
        Some(format!("{list} changed"))
    }

    /// What changed, one name per entry, for the expand.
    pub(crate) fn names(&self) -> Vec<String> {
        self.items
            .iter()
            .map(|a| match a {
                Acted::Note(p) => p.trim_start_matches("memory/").to_owned(),
                Acted::Skill(n) => format!("{n} (skill)"),
                Acted::Instructions(p) => p.clone(),
                Acted::Subagent(n) => format!("agents/{n}.md"),
            })
            .collect()
    }
}

/// What an update changes since the previous one.
#[derive(Clone, Debug, Default)]
pub(crate) struct Review {
    pub hooks: Vec<HookChange>,
    /// Passkeys the machines will start or stop trusting, against the last update.
    pub keys: Vec<KeyChange>,
    /// First update only: every passkey the machines will trust (listed, not a change).
    pub trusted: Vec<String>,
    /// What else changed outside Atlas (or since the last update), for the line on the page.
    pub others: Others,
    /// Everything counted that changed, however it was made, for the line beside the tap.
    pub changed: Others,
}

impl Review {
    /// Whether the owner must look before signing: a hook or a trusted passkey changes.
    pub(crate) fn needs_look(&self) -> bool {
        !self.hooks.is_empty() || !self.keys.is_empty()
    }
}

/// A passkey the update adds to or removes from what machines trust.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct KeyChange {
    /// The name the hub stores for it, such as "Phone".
    pub name: String,
    pub added: bool,
}

impl KeyChange {
    /// "This update adds the passkey 'Phone'".
    pub(crate) fn text(&self) -> String {
        let verb = if self.added { "adds" } else { "removes" };
        format!("This update {verb} the passkey '{}'", self.name)
    }
}

/// The owner keys that differ between two updates. A key counts as the same only when its
/// id, algorithm and public key all match.
fn key_changes(previous: &Manifest, next: &Manifest) -> Vec<KeyChange> {
    let same = |a: &OwnerKey, b: &OwnerKey| {
        a.credential_id == b.credential_id && a.alg == b.alg && a.public_key == b.public_key
    };
    let name = |k: &OwnerKey| {
        if k.name.is_empty() {
            "unnamed".to_owned()
        } else {
            k.name.clone()
        }
    };
    let added = next
        .owner_keys
        .iter()
        .filter(|k| !previous.owner_keys.iter().any(|p| same(p, k)))
        .map(|k| KeyChange {
            name: name(k),
            added: true,
        });
    let removed = previous
        .owner_keys
        .iter()
        .filter(|p| !next.owner_keys.iter().any(|k| same(p, k)))
        .map(|k| KeyChange {
            name: name(k),
            added: false,
        });
    added.chain(removed).collect()
}

/// Plain words for when a hook runs.
pub(crate) fn when(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "When a session starts",
        HookEvent::UserPrompt => "When you send a prompt",
        HookEvent::PreToolUse => "Before a tool runs",
        HookEvent::PostToolUse => "After a tool runs",
        HookEvent::Stop => "When the agent finishes",
    }
}

fn root_of(sources: &Sources, id: &SourceId) -> Option<PathBuf> {
    match id {
        SourceId::Knowledge => Some(sources.knowledge.clone()),
        SourceId::PublicSkills => sources.public_skills.clone(),
        SourceId::Project(_) => None,
    }
}

/// Compares the commits `next` pins with those `previous` pinned. A repo whose previous
/// commit is unknown here is read as if sent for the first time, so its hooks all show.
pub(crate) fn review(sources: &Sources, previous: Option<&Manifest>, next: &Manifest) -> Review {
    let mut out = Review::default();
    match previous {
        Some(p) => out.keys = key_changes(p, next),
        None => out.trusted = next.owner_keys.iter().map(|k| k.name.clone()).collect(),
    }
    let mut outside_known = true;
    for src in &next.sources {
        let Some(repo) = root_of(sources, &src.id).and_then(|r| Repo::open(&r).ok()) else {
            continue;
        };
        let head = src.commit.as_str();
        let base = previous
            .and_then(|p| p.source(&src.id))
            .map(|p| p.commit.as_str().to_owned())
            .filter(|b| repo.has_commit(b));
        let Ok(changed) = repo.changed_paths(base.as_deref(), head) else {
            continue;
        };
        let mut hooks: BTreeSet<String> = BTreeSet::new();
        let mut acted: Vec<(Acted, String)> = Vec::new();
        for c in &changed {
            match classify(&src.id, &c.path) {
                Kind::Runs(Runs::Hook(name)) => {
                    hooks.insert(name);
                }
                Kind::Runs(Runs::HooksFolder) => {
                    if is_link(&repo, head, "hooks") {
                        out.hooks.push(folder_link());
                    }
                }
                Kind::Acted(a) => acted.push((a, c.path.clone())),
                Kind::Other => {}
            }
        }
        for name in &hooks {
            if let Some(h) = hook_change(&repo, base.as_deref(), head, name) {
                out.hooks.push(h);
            }
        }
        for c in &changed {
            if let Some(owner) = with_hooks(&src.id, &c.path)
                && let Some(h) = frontmatter_change(&repo, base.as_deref(), head, &c.path, &owner)
            {
                out.hooks.push(h);
            }
        }
        // The other things count only against an earlier update.
        let Some(base) = base else { continue };
        out.changed
            .items
            .extend(acted.iter().map(|(a, _)| a.clone()));
        if let Some(paths) = outside_paths(&repo, &base, head) {
            acted.retain(|(_, p)| paths.contains(p));
        } else {
            outside_known = false;
        }
        out.others.items.extend(acted.into_iter().map(|(a, _)| a));
    }
    out.others.outside = outside_known;
    out
}

/// Paths touched by commits Atlas didn't make (its commit messages end "in Atlas"). None
/// when that can't be told cheaply.
fn outside_paths(repo: &Repo, base: &str, head: &str) -> Option<BTreeSet<String>> {
    let commits = repo.commits_between(base, head).ok()?;
    let theirs: Vec<&String> = commits
        .iter()
        .filter(|(_, subject)| !subject.ends_with(" in Atlas"))
        .map(|(id, _)| id)
        .collect();
    if theirs.len() > COMMITS_MAX {
        return None;
    }
    let mut paths = BTreeSet::new();
    for id in theirs {
        paths.extend(repo.paths_in_commit(id).ok()?);
    }
    Some(paths)
}

fn text_of(repo: &Repo, commit: &str, path: &str) -> (String, String) {
    match repo.file_at(commit, path, SHOW_MAX) {
        Ok(Some(bytes)) => match String::from_utf8(bytes) {
            Ok(text) => (text, String::new()),
            Err(e) => (
                String::new(),
                format!(
                    "Not text ({} bytes), so it can't be shown here.",
                    e.as_bytes().len()
                ),
            ),
        },
        Ok(None) => (String::new(), "Not a plain file.".to_owned()),
        Err(atlas_git::GitError::TooLarge(_, size)) => (
            String::new(),
            format!("Too large to show here ({size} bytes)."),
        ),
        Err(e) => (String::new(), format!("Couldn't be read: {e}")),
    }
}

/// A hook that exists at `head` and is new or changed since `base`. None for a deleted
/// hook: removing one needs no look.
fn hook_change(repo: &Repo, base: Option<&str>, head: &str, name: &str) -> Option<HookChange> {
    let dir = format!("hooks/{name}");
    if is_link(repo, head, &dir) {
        let target = text_of(repo, head, &dir).0;
        return Some(HookChange {
            name: name.to_owned(),
            new: true,
            link: format!(
                "Its folder is a link (to {target}), so your machines will skip this hook."
            ),
            ..HookChange::blank()
        });
    }
    let toml_path = format!("{dir}/hook.toml");
    let manifest_text = repo.file_at(head, &toml_path, SHOW_MAX).ok()??;
    let entries = repo.tree_entries(head, &dir).unwrap_or_default();
    let had_hook = base.is_some_and(|b| {
        repo.file_at(b, &toml_path, SHOW_MAX)
            .ok()
            .flatten()
            .is_some()
    });
    let manifest: Option<HookManifest> = std::str::from_utf8(&manifest_text)
        .ok()
        .and_then(|t| toml::from_str(t).ok());
    let (description, when_text, matcher, run) = match &manifest {
        Some(m) => (
            m.description.clone(),
            when(m.event).to_owned(),
            m.matcher.clone().unwrap_or_default(),
            m.run.clone(),
        ),
        None => (
            String::new(),
            "Its hook.toml isn't valid, so machines will skip it".to_owned(),
            String::new(),
            String::new(),
        ),
    };
    let script = format!("{dir}/{run}");
    let mode = |path: &str| {
        entries
            .iter()
            .find(|(_, p)| p == path)
            .map(|(m, _)| m.as_str())
    };
    let links: Vec<&str> = entries
        .iter()
        .filter(|(m, _)| m == LINK)
        .map(|(_, p)| p.trim_start_matches(&format!("{dir}/")))
        .collect();
    let link = if mode(&script) == Some(LINK) {
        "Its script is a link, so your machines will skip this hook.".to_owned()
    } else if let Some(first) = links.first() {
        format!("Its folder holds a link ({first}), so your machines will skip this hook.")
    } else {
        String::new()
    };
    let mut change = HookChange {
        name: name.to_owned(),
        new: !had_hook,
        description,
        when: when_text,
        matcher,
        not_runnable: !run.is_empty() && mode(&script).is_some_and(|m| m == "100644"),
        link,
        ..HookChange::blank()
    };
    if let (true, Some(base)) = (had_hook, base) {
        change.diff = parse_diff(&repo.diff(base, head, &dir).unwrap_or_default(), &dir);
        return Some(change);
    }
    let mut files: Vec<&(String, String)> =
        entries.iter().filter(|(_, f)| *f != toml_path).collect();
    // The script first.
    files.sort_by_key(|(_, f)| (*f != script, f.clone()));
    for (mode, path) in files {
        let (text, note) = text_of(repo, head, path);
        let (text, note) = if mode == LINK {
            (String::new(), format!("A link to {text}"))
        } else {
            (text, note)
        };
        change.files.push(HookFile {
            name: path.trim_start_matches(&format!("{dir}/")).to_owned(),
            text,
            note,
        });
    }
    if change.files.is_empty() {
        change.files.push(HookFile {
            name: run,
            text: String::new(),
            note: "Its script is missing, so machines will skip it.".to_owned(),
        });
    }
    Some(change)
}

/// The tree mode of a symbolic link.
const LINK: &str = "120000";

fn is_link(repo: &Repo, commit: &str, path: &str) -> bool {
    repo.mode_at(commit, path)
        .ok()
        .flatten()
        .is_some_and(|m| m == LINK)
}

impl HookChange {
    fn blank() -> Self {
        Self {
            name: String::new(),
            place: String::new(),
            new: false,
            description: String::new(),
            when: String::new(),
            matcher: String::new(),
            files: Vec::new(),
            diff: Vec::new(),
            not_runnable: false,
            link: String::new(),
        }
    }
}

/// The whole `hooks` folder is a link: every hook would follow it.
fn folder_link() -> HookChange {
    HookChange {
        name: "hooks".to_owned(),
        new: true,
        link: "The hooks folder is a link, so your machines will skip every hook.".to_owned(),
        ..HookChange::blank()
    }
}

/// The `hooks` block of a markdown file's frontmatter, as written (`hooks:` at the start
/// of a line, with everything indented or listed under it). None without one.
pub(crate) fn frontmatter_hooks(text: &str) -> Option<String> {
    let (Some(fm), _) = atlas_core::catalog::split_frontmatter(text).ok()? else {
        return None;
    };
    let mut block: Vec<&str> = Vec::new();
    for line in fm.0.lines() {
        if block.is_empty() {
            let key = line.trim_start_matches(['"', '\'']);
            if line == line.trim_start()
                && key.starts_with("hooks")
                && key["hooks".len()..]
                    .trim_start_matches(['"', '\''])
                    .trim_start()
                    .starts_with(':')
            {
                block.push(line.trim_end());
            }
            continue;
        }
        let top_level =
            !line.is_empty() && !line.starts_with([' ', '\t', '-', '#']) && line.trim() != "";
        if top_level {
            break;
        }
        block.push(line.trim_end());
    }
    while block.last().is_some_and(|l| l.trim().is_empty()) {
        block.pop();
    }
    (!block.is_empty()).then(|| block.join("\n"))
}

/// Hooks a skill or subagent declares in its frontmatter that are new or changed.
fn frontmatter_change(
    repo: &Repo,
    base: Option<&str>,
    head: &str,
    path: &str,
    owner: &WithHooks,
) -> Option<HookChange> {
    let read = |commit: &str| {
        let bytes = repo.file_at(commit, path, SHOW_MAX).ok()??;
        frontmatter_hooks(&String::from_utf8(bytes).ok()?)
    };
    let now = read(head)?;
    let before = base.and_then(read);
    if before.as_deref() == Some(now.as_str()) {
        return None;
    }
    let (name, place, when_text) = match owner {
        WithHooks::Skill(n) => (n, format!("in the skill {n}"), "While the skill is in use"),
        WithHooks::Subagent(n) => (
            n,
            format!("in the subagent {n}"),
            "While the subagent is in use",
        ),
    };
    let file = path.rsplit('/').next().unwrap_or(path).to_owned();
    let mut change = HookChange {
        name: name.clone(),
        place,
        new: before.is_none(),
        when: when_text.to_owned(),
        ..HookChange::blank()
    };
    match before {
        None => change.files.push(HookFile {
            name: file,
            text: now,
            note: String::new(),
        }),
        Some(old) => {
            change.diff.push(DiffLine {
                class: "file",
                text: file,
            });
            change.diff.extend(line_diff(&old, &now));
        }
    }
    Some(change)
}

/// A line-by-line diff of two short texts (longest common subsequence).
fn line_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let line = |class, mark: char, text: &str| DiffLine {
        class,
        text: format!("{mark}{text}"),
    };
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            out.push(line("ctx", ' ', a[i]));
            i += 1;
            j += 1;
        } else if j < b.len() && (i == a.len() || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(line("add", '+', b[j]));
            j += 1;
        } else {
            out.push(line("del", '-', a[i]));
            i += 1;
        }
    }
    out
}

/// Turns `git diff` output into lines to show, file names relative to the hook's folder.
fn parse_diff(text: &str, dir: &str) -> Vec<DiffLine> {
    let mut out = Vec::new();
    let mut old_name = String::new();
    let short = |p: &str| {
        p.trim_start_matches("a/")
            .trim_start_matches("b/")
            .trim_start_matches(&format!("{dir}/"))
            .to_owned()
    };
    for line in text.lines() {
        if let Some(p) = line.strip_prefix("--- ") {
            old_name = short(p);
        } else if let Some(p) = line.strip_prefix("+++ ") {
            let text = if p == "/dev/null" {
                format!("{old_name} · removed")
            } else if old_name == "/dev/null" {
                format!("{} · new", short(p))
            } else {
                short(p)
            };
            out.push(DiffLine {
                class: "file",
                text,
            });
        } else if line.starts_with("diff --git")
            || line.starts_with("index ")
            || line.starts_with("new file mode")
            || line.starts_with("deleted file mode")
            || line.starts_with("old mode")
            || line.starts_with("new mode")
            || line.starts_with("similarity")
        {
        } else if line.starts_with("Binary files") {
            out.push(DiffLine {
                class: "hunk",
                text: "Not text, so the change can't be shown here.".to_owned(),
            });
        } else if line.starts_with("@@") {
            out.push(DiffLine {
                class: "hunk",
                text: line.to_owned(),
            });
        } else {
            let class = match line.as_bytes().first() {
                Some(b'+') => "add",
                Some(b'-') => "del",
                _ => "ctx",
            };
            out.push(DiffLine {
                class,
                text: line.to_owned(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_sort_into_what_runs_and_what_agents_act_on() {
        let k = SourceId::Knowledge;
        assert_eq!(
            classify(&k, "hooks/guard/run.sh"),
            Kind::Runs(Runs::Hook("guard".into()))
        );
        assert_eq!(
            classify(&k, "hooks/guard/hook.toml"),
            Kind::Runs(Runs::Hook("guard".into()))
        );
        assert_eq!(classify(&k, "hooks"), Kind::Runs(Runs::HooksFolder));
        assert_eq!(
            classify(&k, "hooks/linked"),
            Kind::Runs(Runs::Hook("linked".into()))
        );
        assert_eq!(
            classify(&k, "skills/pdf/SKILL.md"),
            Kind::Acted(Acted::Skill("pdf".into()))
        );
        assert_eq!(
            classify(&SourceId::PublicSkills, "pdf/scripts/x.py"),
            Kind::Acted(Acted::Skill("pdf".into()))
        );
        assert_eq!(classify(&SourceId::PublicSkills, "README.md"), Kind::Other);
        assert_eq!(
            classify(&k, "memory/hosts/pi/a.md"),
            Kind::Acted(Acted::Note("memory/hosts/pi/a.md".into()))
        );
        assert_eq!(
            classify(&k, "AGENTS.md"),
            Kind::Acted(Acted::Instructions("AGENTS.md".into()))
        );
        assert_eq!(
            classify(&k, "agents/reviewer.md"),
            Kind::Acted(Acted::Subagent("reviewer".into()))
        );
        assert_eq!(classify(&k, "atlas/placements.toml"), Kind::Other);
    }

    #[test]
    fn the_other_things_line_counts_and_names() {
        let mut o = Others::default();
        assert_eq!(o.line(), None);
        o.items.insert(Acted::Note("memory/a.md".into()));
        o.items.insert(Acted::Note("memory/b.md".into()));
        o.items.insert(Acted::Note("memory/c.md".into()));
        o.items.insert(Acted::Skill("pdf".into()));
        o.outside = true;
        assert_eq!(
            o.line().unwrap(),
            "3 notes and 1 skill changed outside Atlas"
        );
        o.items.insert(Acted::Instructions("AGENTS.md".into()));
        o.outside = false;
        assert_eq!(
            o.line().unwrap(),
            "3 notes, 1 skill and 1 instruction file changed since the last update"
        );
        o.items.insert(Acted::Subagent("reviewer".into()));
        assert_eq!(
            o.line().unwrap(),
            "3 notes, 1 skill, 1 instruction file and 1 subagent file changed since the last update"
        );
        assert!(o.names().contains(&"agents/reviewer.md".to_owned()));
        assert!(o.names().contains(&"pdf (skill)".to_owned()));
    }

    #[test]
    fn frontmatter_hooks_are_read_as_written() {
        let skill = "---\nname: pdf\nhooks:\n  PreToolUse:\n    - matcher: Bash\n      hooks:\n        - type: command\n          command: ./check.sh\ndescription: x\n---\nBody\nhooks: not this\n";
        assert_eq!(
            frontmatter_hooks(skill).unwrap(),
            "hooks:\n  PreToolUse:\n    - matcher: Bash\n      hooks:\n        - type: command\n          command: ./check.sh"
        );
        assert_eq!(
            frontmatter_hooks("---\n\"hooks\": {Stop: []}\n---\n").unwrap(),
            "\"hooks\": {Stop: []}"
        );
        assert_eq!(frontmatter_hooks("---\nname: a\n---\nhooks:\n  x\n"), None);
        assert_eq!(frontmatter_hooks("---\nhooksy: 1\n---\n"), None);
        assert_eq!(frontmatter_hooks("no frontmatter"), None);
    }

    #[test]
    fn a_line_diff_marks_what_changed() {
        let lines = line_diff("a\nb\nc", "a\nB\nc\nd");
        let got: Vec<_> = lines.iter().map(|l| (l.class, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                ("ctx", " a"),
                ("add", "+B"),
                ("del", "-b"),
                ("ctx", " c"),
                ("add", "+d"),
            ]
        );
    }

    #[test]
    fn the_line_beside_the_tap_names_what_changed() {
        let mut o = Others::default();
        assert_eq!(o.summary(), None);
        o.items.insert(Acted::Note("memory/a.md".into()));
        assert_eq!(o.summary().unwrap(), "1 note changed");
        o.items.insert(Acted::Instructions("AGENTS.md".into()));
        o.items.insert(Acted::Skill("a".into()));
        o.items.insert(Acted::Skill("b".into()));
        o.items.remove(&Acted::Note("memory/a.md".into()));
        assert_eq!(o.summary().unwrap(), "AGENTS.md and 2 skills changed");
    }

    #[test]
    fn a_diff_reads_as_lines_with_file_names() {
        let text = "diff --git a/hooks/g/run.sh b/hooks/g/run.sh\nindex 1..2 100644\n--- a/hooks/g/run.sh\n+++ b/hooks/g/run.sh\n@@ -1 +1 @@\n-echo old\n+echo new\n same\n";
        let lines = parse_diff(text, "hooks/g");
        let got: Vec<_> = lines.iter().map(|l| (l.class, l.text.as_str())).collect();
        assert_eq!(
            got,
            vec![
                ("file", "run.sh"),
                ("hunk", "@@ -1 +1 @@"),
                ("del", "-echo old"),
                ("add", "+echo new"),
                ("ctx", " same"),
            ]
        );
    }
}
