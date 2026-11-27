//! One pass of `atlasd run`: everything after the release has been fetched.

use std::path::PathBuf;

use atlas_core::distribution::Report;
use atlas_core::release::{SignedRelease, SourceId};

use crate::config::{Config, Paths, State};
use crate::sources::Copies;
use crate::{desired, plan, release, report, sources};

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
    let mut lines = Vec::new();
    let mut problems = Vec::new();
    let mut failed = false;
    let mut next = state.clone();

    match fetched {
        Fetched::Unreachable(e) => {
            let at = state.last_applied.map_or_else(
                || "nothing applied yet".to_owned(),
                |n| format!("release {n}"),
            );
            problems.push(format!(
                "couldn't reach the hub ({e}); harnesses keep using the local copies ({at})"
            ));
        }
        Fetched::Nothing => lines.push("The hub has no signed release yet.".to_owned()),
        Fetched::Release(signed) => match release::check(&signed, state, &config.hub) {
            Err(e) => {
                failed = true;
                problems.push(format!("refused release {}: {e}", signed.number));
            }
            Ok(checked) => {
                let n = checked.manifest.number;
                let copies = if dry_run {
                    let (copies, notes) = sources::existing(&checked.manifest, &paths.sources);
                    lines.extend(notes);
                    Ok(copies)
                } else {
                    sources::sync(&checked.manifest, &paths.sources)
                };
                match copies {
                    Err(e) => {
                        failed = true;
                        problems.push(format!("refused release {n}: {e}"));
                    }
                    Ok(copies) => {
                        let ok = apply_release(
                            paths,
                            config,
                            &copies,
                            dry_run,
                            &mut lines,
                            &mut problems,
                        );
                        if !ok {
                            failed = true;
                        } else if !dry_run {
                            next.last_applied = Some(n);
                            next.last_digest = Some(checked.digest);
                            if checked.newer {
                                next.owner_keys = checked.manifest.owner_keys;
                            }
                            lines.push(format!("Release {n} is applied."));
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
        problems: problems.clone(),
    };
    Pass {
        lines,
        problems,
        failed,
        state: next,
        report,
    }
}

/// Plans from the copies and, unless a dry run, applies. False when a change failed.
fn apply_release(
    paths: &Paths,
    config: &Config,
    copies: &Copies,
    dry_run: bool,
    lines: &mut Vec<String>,
    problems: &mut Vec<String>,
) -> bool {
    problems.extend(copies.problems.iter().cloned());
    let knowledge = copies.dirs.get(&SourceId::Knowledge);
    if dry_run && knowledge.is_none() {
        lines.push(
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
    problems.extend(desired.problems.iter().cloned());

    let mut roots = vec![paths.sources.clone()];
    roots.extend(config.adopt_roots(&paths.home));
    let plan = plan::plan(&paths.home, &desired, &roots);
    problems.extend(plan.conflicts.iter().cloned());

    if dry_run {
        if plan.actions.is_empty() {
            lines.push("Nothing to change.".to_owned());
        }
        lines.extend(
            plan.actions
                .iter()
                .map(|a| format!("would {}", a.describe(&paths.home))),
        );
        return true;
    }
    let (done, errors) = crate::apply::apply(&plan, &paths.home);
    lines.extend(done);
    let ok = errors.is_empty();
    problems.extend(errors);
    ok
}
