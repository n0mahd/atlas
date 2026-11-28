//! One pass of `atlasd run`: everything after the release has been fetched.
//!
//! Releases are "updates" in everything a person reads, and harnesses are "agents".

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use atlas_core::distribution::Report;
use atlas_core::release::{SignedRelease, SourceId};

use crate::config::{Config, Paths, State};
use crate::plan::Action;
use crate::sources::Copies;
use crate::{desired, display, plan, release, report, sources};

/// What asking the hub for its latest release gave.
#[derive(Clone, Debug)]
pub enum Fetched {
    Release(Box<SignedRelease>),
    /// The hub has no signed release yet.
    Nothing,
    /// The hub couldn't be reached; the local copies stay as they are.
    Unreachable(String),
}

/// What a pass did.
#[derive(Clone, Debug)]
pub struct Pass {
    /// What was done (or, in a dry run, would be), in words.
    pub lines: Vec<String>,
    /// Refusals, failures and conflicts, in words. Also in `report.problems`.
    pub problems: Vec<String>,
    /// The problems that are only things Atlas left alone because they aren't its own.
    pub left_alone: Vec<String>,
    /// Two or three lines for `atlasd run --summary`: what was linked for which agents,
    /// and whether anything of the person's was replaced. Empty when no update was
    /// applied (the problems say why).
    pub summary: Vec<String>,
    /// A release was refused or a change failed.
    pub failed: bool,
    /// The state to keep: changed only when a release applied.
    pub state: State,
    pub report: Report,
}

/// Runs one pass with `fetched` as the hub's answer.
///
/// A release is applied only if [`release::check`] accepts it and the knowledge and
/// public skills copies match their pinned trees. Its number and owner keys are kept
/// only once every change in the plan succeeded. A dry run fetches nothing, changes
/// nothing and plans from the local copies as they stand.
#[must_use]
pub fn run(
    paths: &Paths,
    config: &Config,
    state: &State,
    fetched: Fetched,
    dry_run: bool,
    now: u64,
) -> Pass {
    let mut out = Outcome::default();
    let mut failed = false;
    let mut next = state.clone();

    match fetched {
        Fetched::Unreachable(e) => {
            let at = state.last_applied.map_or_else(
                || "nothing applied yet".to_owned(),
                |n| format!("update {n}"),
            );
            out.problems.push(format!(
                "couldn't reach the hub ({e}); agents keep using the local copies ({at})"
            ));
        }
        Fetched::Nothing => {
            let line = "The hub has no signed update yet, so there is nothing to link.";
            out.lines.push(line.to_owned());
            out.summary.push(line.to_owned());
        }
        Fetched::Release(signed) => match release::check(&signed, state, &config.hub) {
            Err(e) => {
                failed = true;
                out.problems
                    .push(format!("refused update {}: {e}", signed.number));
            }
            Ok(checked) => {
                let n = checked.manifest.number;
                let copies = if dry_run {
                    let (copies, notes) = sources::existing(&checked.manifest, &paths.sources);
                    out.lines.extend(notes);
                    Ok(copies)
                } else {
                    sources::sync(&checked.manifest, &paths.sources)
                };
                match copies {
                    Err(e) => {
                        failed = true;
                        out.problems.push(format!("refused update {n}: {e}"));
                    }
                    Ok(copies) => {
                        let ok = apply_release(paths, config, &copies, n, dry_run, &mut out);
                        if !ok {
                            failed = true;
                        } else if !dry_run {
                            next.last_applied = Some(n);
                            next.last_digest = Some(checked.digest);
                            if checked.newer {
                                next.owner_keys = checked.manifest.owner_keys;
                            }
                            out.lines.push(format!("Update {n} is applied."));
                        }
                    }
                }
            }
        },
    }

    let report = Report {
        release: next.last_applied,
        at: now,
        harnesses: report::harnesses(&paths.home, &paths.sources),
        problems: out.problems.clone(),
    };
    Pass {
        lines: out.lines,
        problems: out.problems,
        left_alone: out.left_alone,
        summary: out.summary,
        failed,
        state: next,
        report,
    }
}

#[derive(Default)]
struct Outcome {
    lines: Vec<String>,
    problems: Vec<String>,
    left_alone: Vec<String>,
    summary: Vec<String>,
}

/// Plans from the copies and, unless a dry run, applies. False when a change failed.
fn apply_release(
    paths: &Paths,
    config: &Config,
    copies: &Copies,
    number: u64,
    dry_run: bool,
    out: &mut Outcome,
) -> bool {
    out.problems.extend(copies.problems.iter().cloned());
    let knowledge = copies.dirs.get(&SourceId::Knowledge);
    if dry_run && knowledge.is_none() {
        out.lines.push(
            "Nothing to plan without a local copy of the knowledge repo; run `atlasd run` once."
                .to_owned(),
        );
        return true;
    }
    let desired = desired::read(
        knowledge.map(PathBuf::as_path),
        copies
            .dirs
            .get(&SourceId::PublicSkills)
            .map(PathBuf::as_path),
    );
    out.problems.extend(desired.problems.iter().cloned());

    let mut roots = vec![paths.sources.clone()];
    roots.extend(config.adopt_roots(&paths.home));
    let plan = plan::plan(&paths.home, &desired, &roots);
    out.problems.extend(plan.conflicts.iter().cloned());
    out.left_alone.extend(plan.conflicts.iter().cloned());

    if dry_run {
        if plan.actions.is_empty() {
            out.lines.push("Nothing to change.".to_owned());
        }
        out.lines.extend(
            plan.actions
                .iter()
                .map(|a| format!("would {}", a.describe(&paths.home))),
        );
        return true;
    }
    let (done, errors) = crate::apply::apply(&plan, &paths.home);
    out.lines
        .extend(done.iter().map(|a| a.describe(&paths.home)));
    out.summary = summarise(
        &paths.home,
        &paths.sources,
        number,
        &done,
        plan.conflicts.len(),
        !errors.is_empty(),
    );
    let ok = errors.is_empty();
    out.problems.extend(errors);
    ok
}

/// "a, b and c".
fn and_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// The `--summary` lines, from the actions that were carried out.
fn summarise(
    home: &Path,
    sources: &Path,
    number: u64,
    done: &[&Action],
    left_alone: usize,
    partly: bool,
) -> Vec<String> {
    let agents: Vec<_> = plan::present(home)
        .into_iter()
        .filter(|l| !l.kind.is_host())
        .collect();
    if agents.is_empty() {
        return vec![format!(
            "Update {number} has nothing to link here: no agents are set up for this login yet. atlasd links them on its next run after you install one."
        )];
    }
    let agent_dirs: Vec<&str> = agents.iter().filter_map(|l| l.agent_dir).collect();
    let skill_dirs: Vec<&str> = agents
        .iter()
        .flat_map(|l| l.skill_dirs.iter().copied())
        .collect();
    let in_one = |path: &Path, dirs: &[&str]| {
        path.parent()
            .is_some_and(|parent| dirs.iter().any(|d| parent == home.join(d)))
    };
    let is_instruction = |path: &Path| {
        agents
            .iter()
            .any(|l| l.instruction_files.iter().any(|f| path == home.join(f)))
    };
    let name = |path: &Path| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let mut skills = BTreeSet::new();
    let mut subagents = BTreeSet::new();
    let mut instructions = false;
    let mut hooks = false;
    let mut imports = Vec::new();
    let mut replaced = Vec::new();
    let mut removed = Vec::new();
    for action in done {
        match action {
            Action::CreateLink { path, .. } | Action::ReplaceLink { path, .. } => {
                if is_instruction(path) {
                    instructions = true;
                } else if in_one(path, &agent_dirs) {
                    subagents.insert(name(path));
                } else if in_one(path, &skill_dirs) {
                    skills.insert(name(path));
                }
                if let Action::ReplaceLink { from, .. } = action
                    && !from.starts_with(sources)
                {
                    replaced.push(format!(
                        "{} (was a link to {})",
                        display(home, path),
                        display(home, from)
                    ));
                }
            }
            Action::RemoveLink { path, target } if !target.starts_with(sources) => {
                removed.push(display(home, path));
            }
            Action::WriteFile { path, .. } if is_instruction(path) => {
                instructions = true;
                imports.push(display(home, path));
            }
            Action::WriteFile { .. } | Action::RemoveFile { .. } => hooks = true,
            Action::RemoveLink { .. } | Action::CreateDir(_) => {}
        }
    }

    let names: Vec<String> = agents.iter().map(|l| l.kind.to_string()).collect();
    let mut linked = Vec::new();
    if !skills.is_empty() {
        linked.push(plural(skills.len(), "skill", "skills"));
    }
    if !subagents.is_empty() {
        linked.push(plural(subagents.len(), "subagent", "subagents"));
    }
    if instructions {
        linked.push("the global instructions".to_owned());
    }
    let what = match (linked.is_empty(), hooks) {
        (true, false) => "everything was already in place".to_owned(),
        (true, true) => "updated Atlas's hooks".to_owned(),
        (false, false) => format!("linked {}", and_list(&linked)),
        (false, true) => format!("linked {}, and updated Atlas's hooks", and_list(&linked)),
    };
    let applied = if partly { "Partly applied" } else { "Applied" };
    let mut lines = vec![format!(
        "{applied} update {number} for {}: {what}.",
        and_list(&names)
    )];

    let first = |items: &[String]| {
        let shown: Vec<String> = items.iter().take(3).cloned().collect();
        match items.len() {
            n if n > 3 => format!("{} and {} more", shown.join(", "), n - 3),
            _ => and_list(&shown),
        }
    };
    let mut yours = Vec::new();
    if !replaced.is_empty() {
        yours.push(format!(
            "Replaced by links to Atlas's copies: {}.",
            first(&replaced)
        ));
    }
    if !removed.is_empty() {
        yours.push(format!(
            "Removed older links no longer wanted: {}.",
            first(&removed)
        ));
    }
    let import = (!imports.is_empty())
        .then(|| format!("; {} gained one import line", and_list(&imports)))
        .unwrap_or_default();
    if yours.is_empty() {
        lines.push(format!("Nothing of yours was deleted or replaced{import}."));
    } else {
        lines.push(format!(
            "{} Nothing else of yours was deleted{import}.",
            yours.join(" ")
        ));
    }
    if left_alone > 0 {
        lines.push(format!(
            "Left {} alone because Atlas doesn't manage {}; `atlasd status` lists {}.",
            plural(left_alone, "thing", "things"),
            if left_alone == 1 { "it" } else { "them" },
            if left_alone == 1 { "it" } else { "them" },
        ));
    }
    lines
}
