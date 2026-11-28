//! Tidy up for skills: one page that gathers what could be in better shape, on this machine
//! and in each registered project, and fixes the chosen items in one pass. Adapted from T3
//! Code's Tidy up (`SkillTidy.logic.ts`):
//! - bring outside skills into Atlas, or leave them outside (`[keep]`);
//! - drop a project's copy of a skill that is also Global (with Compare);
//! - move skills out of a project's Claude-only `.claude/skills` into `.agents/skills`, where
//!   every agent finds them (atlasd links them back for Claude Code);
//! - turn project skills back on for agents they were switched off for.
//!
//! Nothing changes until the owner confirms. Then it is one change: Knowledge first (it
//! gains the copies), then each project, one commit per repo.

use std::collections::BTreeSet;

use askama::Template;
use atlas_core::catalog::SkillHome;
use atlas_core::catalog::edit::RepoKind;
use atlas_core::kernel::{HarnessKind, Scope};
use atlas_fs::library;
use axum::Form;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::import::{self, Plan, Write, plural};
use crate::views::{Page, Snapshot};
use crate::{AppError, AppState, change};

#[derive(Debug)]
pub struct OutsideChoice {
    /// The radio group's name.
    pub field: String,
    pub key: String,
    pub name: String,
    pub source: String,
    /// What a copy leaves out, when anything.
    pub left_out: String,
}

#[derive(Debug)]
pub struct Choice {
    /// The checkbox value: `project/name` or `project/name/agent`.
    pub id: String,
    pub name: String,
    pub note: String,
    pub compare: Option<String>,
    pub disabled: bool,
}

#[derive(Debug)]
pub struct ProjectTidy {
    pub num: String,
    pub name: String,
    pub duplicates: Vec<Choice>,
    pub claude_only: Vec<Choice>,
    pub switched_off: Vec<Choice>,
}

#[derive(Debug)]
pub struct TidyView {
    pub outside: Vec<OutsideChoice>,
    /// Outside skills Atlas has under the same name with other files: they need Compare.
    pub differs: Vec<(String, String)>,
    pub projects: Vec<ProjectTidy>,
}

impl TidyView {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outside.is_empty() && self.differs.is_empty() && self.projects.is_empty()
    }
}

/// How many things Tidy up would offer to fix (switched-off skills are the owner's choice,
/// so they don't count towards the offer).
#[must_use]
pub(crate) fn count(snap: &Snapshot) -> usize {
    let o = &snap.outside;
    o.skills
        .iter()
        .filter(|s| !s.kept && s.origin.importable() && s.clash.as_ref().is_none_or(|c| !c.same))
        .count()
        + o.duplicates.len()
        + o.claude_only.iter().filter(|c| !c.taken).count()
}

#[must_use]
pub(crate) fn view(snap: &Snapshot) -> TidyView {
    let o = &snap.outside;
    let outside: Vec<OutsideChoice> = o
        .skills
        .iter()
        .filter(|s| !s.kept && s.blocked().is_none() && s.clash.is_none())
        .enumerate()
        .map(|(i, s)| OutsideChoice {
            field: format!("r{i}"),
            key: s.key.clone(),
            name: s.name.clone(),
            source: s.entries.join(", "),
            left_out: s.left_out.join(", "),
        })
        .collect();
    let differs: Vec<(String, String)> = o
        .skills
        .iter()
        .filter(|s| !s.kept && s.blocked().is_none() && s.clash.as_ref().is_some_and(|c| !c.same))
        .map(|s| {
            (
                s.name.clone(),
                format!("/import/compare?key={}", import::query(&s.key)),
            )
        })
        .collect();
    let mut projects: Vec<ProjectTidy> = snap
        .projects
        .iter()
        .filter_map(|p| project_tidy(o, &p.project))
        .collect();
    let first = if outside.is_empty() && differs.is_empty() {
        1
    } else {
        2
    };
    for (i, p) in projects.iter_mut().enumerate() {
        p.num = import::roman(first + i);
    }
    TidyView {
        outside,
        differs,
        projects,
    }
}

/// What Tidy up offers in one project, if anything.
fn project_tidy(
    o: &atlas_fs::outside::Outside,
    project: &atlas_core::kernel::Slug,
) -> Option<ProjectTidy> {
    let name = project.as_str();
    let duplicates: Vec<Choice> = o
        .duplicates
        .iter()
        .filter(|d| d.project == *project)
        .map(|d| Choice {
            id: format!("{name}/{}", d.name),
            name: d.name.to_string(),
            note: if d.same {
                "The same files as Global's".to_owned()
            } else {
                "Differs from Global's".to_owned()
            },
            compare: Some(format!(
                "/import/compare-copies?project={name}&name={}",
                d.name
            )),
            disabled: false,
        })
        .collect();
    let claude_only: Vec<Choice> = o
        .claude_only
        .iter()
        .filter(|c| c.project == *project)
        .map(|c| Choice {
            id: format!("{name}/{}", c.name),
            name: c.name.clone(),
            note: if c.taken {
                ".agents/skills has one of that name already; sort it out by hand".to_owned()
            } else {
                "In .claude/skills, which only Claude-style agents read".to_owned()
            },
            compare: None,
            disabled: c.taken,
        })
        .collect();
    let switched_off: Vec<Choice> = o
        .switched_off
        .iter()
        .filter(|x| x.project == *project)
        .flat_map(|x| {
            x.agents.iter().map(move |&agent| Choice {
                id: format!("{name}/{}/{}", x.name, agent.id()),
                name: x.name.to_string(),
                note: format!("Off for {}", crate::views::short(agent)),
                compare: None,
                disabled: false,
            })
        })
        .collect();
    if duplicates.is_empty() && claude_only.is_empty() && switched_off.is_empty() {
        return None;
    }
    Some(ProjectTidy {
        num: String::new(),
        name: name.to_owned(),
        duplicates,
        claude_only,
        switched_off,
    })
}

#[derive(Template)]
#[template(path = "tidy.html")]
struct TidyPage {
    page: Page,
    v: TidyView,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct TidyQuery {
    #[serde(default)]
    saved: String,
    #[serde(default)]
    unchanged: Option<String>,
    #[serde(default)]
    sent: Option<String>,
}

pub(crate) async fn page(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<TidyQuery>,
) -> Result<Html<String>, AppError> {
    let snap = s.snapshot().await?;
    let v = view(&snap);
    let mut page = s.page(&headers, "/skills", "Tidy up", &snap, &Scope::Global, false);
    page.flash = if q.unchanged.is_some() {
        "Nothing was chosen, so nothing changed.".to_owned()
    } else if q.sent.is_some() {
        "Tidied · on your machines within 10 minutes".to_owned()
    } else if !q.saved.is_empty() && q.saved.bytes().all(|b| b.is_ascii_hexdigit()) {
        "Tidied and committed.".to_owned()
    } else {
        String::new()
    };
    Ok(Html(TidyPage { page, v }.render()?))
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct CopiesQuery {
    #[serde(default)]
    project: String,
    #[serde(default)]
    name: String,
}

pub(crate) async fn compare_copies(
    State(s): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<CopiesQuery>,
) -> Result<Html<String>, AppError> {
    import::compare_copies(&s, &headers, &q.project, &q.name).await
}

/// What the owner ticked.
#[derive(Debug, Default)]
struct Picked {
    csrf: String,
    bring: Vec<String>,
    leave: Vec<String>,
    drop: BTreeSet<String>,
    share: BTreeSet<String>,
    on: BTreeSet<String>,
}

fn picked(fields: Vec<(String, String)>) -> Picked {
    let mut p = Picked::default();
    for (k, v) in fields {
        match k.as_str() {
            "csrf" => p.csrf = v,
            "drop" => {
                p.drop.insert(v);
            }
            "share" => {
                p.share.insert(v);
            }
            "on" => {
                p.on.insert(v);
            }
            _ if k.starts_with('r') => {
                if let Some(key) = v.strip_prefix("bring:") {
                    p.bring.push(key.to_owned());
                } else if let Some(key) = v.strip_prefix("leave:") {
                    p.leave.push(key.to_owned());
                }
            }
            _ => {}
        }
    }
    p
}

const STALE: &str =
    "Something on this page changed since it was opened. Reload and choose again; nothing changed.";

/// Runs the chosen items as one change.
#[allow(clippy::too_many_lines)]
pub(crate) async fn run(
    State(s): State<AppState>,
    headers: HeaderMap,
    Form(fields): Form<Vec<(String, String)>>,
) -> Response {
    let p = picked(fields);
    if let Some(r) = import::rejected(&s, &headers, &p.csrf) {
        return r;
    }
    let snap = match import::fresh(&s).await {
        Ok(snap) => snap,
        Err(e) => return e.into_response(),
    };
    let o = &snap.outside;

    let mut kwrites = Vec::new();
    let mut parts = Vec::new();
    let mut names = BTreeSet::new();
    for key in &p.bring {
        let Some(item) = o
            .skills
            .iter()
            .find(|x| &x.key == key && !x.kept && x.blocked().is_none() && x.clash.is_none())
        else {
            return import::refuse(&headers, STALE);
        };
        if !names.insert(item.name.clone()) {
            continue;
        }
        let tree = match import::read_source(&s, item) {
            Ok(t) => t,
            Err(m) => return import::refuse(&headers, m),
        };
        kwrites.push(Write::Tree {
            rel: format!("skills/{}", item.name),
            tree,
            replace: false,
        });
    }
    if !names.is_empty() {
        parts.push(format!(
            "bring {} into Atlas",
            plural(names.len(), "skill", "skills")
        ));
    }
    for key in &p.leave {
        if !import::known_key(&snap, key) {
            return import::refuse(&headers, STALE);
        }
    }
    if !p.leave.is_empty() {
        parts.push(format!("leave {} outside", p.leave.len()));
    }

    // Per project: what goes and what moves; placements follow in Knowledge.
    let mut drop_homes = Vec::new();
    let mut on_items: Vec<(SkillHome, atlas_core::kernel::Slug, HarnessKind)> = Vec::new();
    let mut plans_after = Vec::new();
    // Every ticked box must still name something; else the page was stale.
    let mut matched = 0;
    for project in &s.sources.projects {
        let pname = project.name.as_str();
        let mut writes = Vec::new();
        let mut dropped = 0;
        let mut moved = 0;
        for d in o.duplicates.iter().filter(|d| d.project == project.name) {
            if p.drop.contains(&format!("{pname}/{}", d.name)) {
                writes.push(Write::Remove {
                    rel: format!(".agents/skills/{}", d.name),
                });
                drop_homes.push((SkillHome::Project(project.name.clone()), d.name.clone()));
                dropped += 1;
                matched += 1;
            }
        }
        for c in o.claude_only.iter().filter(|c| c.project == project.name) {
            if !p.share.contains(&format!("{pname}/{}", c.name)) {
                continue;
            }
            if c.taken {
                return import::refuse(&headers, STALE);
            }
            let from = project.root.join(".claude/skills").join(&c.name);
            let tree = match library::read_tree(&from) {
                Ok(t) => t,
                Err(e) => {
                    return import::refuse(
                        &headers,
                        format!("{pname}'s {} can't be read: {e}.", c.name),
                    );
                }
            };
            if !tree.skipped.is_empty() {
                return import::refuse(
                    &headers,
                    format!(
                        "{pname}'s .claude/skills/{} holds links or a .git ({}), which a move would lose. Move it by hand.",
                        c.name,
                        tree.skipped
                            .iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                );
            }
            writes.push(Write::Tree {
                rel: format!(".agents/skills/{}", c.name),
                tree,
                replace: false,
            });
            writes.push(Write::Remove {
                rel: format!(".claude/skills/{}", c.name),
            });
            moved += 1;
            matched += 1;
        }
        for x in o.switched_off.iter().filter(|x| x.project == project.name) {
            for &agent in &x.agents {
                if p.on.contains(&format!("{pname}/{}/{}", x.name, agent.id())) {
                    on_items.push((
                        SkillHome::Project(project.name.clone()),
                        x.name.clone(),
                        agent,
                    ));
                    matched += 1;
                }
            }
        }
        if writes.is_empty() {
            continue;
        }
        let mut what = Vec::new();
        if dropped > 0 {
            what.push(format!(
                "drop {} also in Global",
                plural(dropped, "copy", "copies")
            ));
        }
        if moved > 0 {
            what.push(format!(
                "move {} to .agents/skills",
                plural(moved, "skill", "skills")
            ));
        }
        plans_after.push(Plan {
            kind: RepoKind::Project(project.name.clone()),
            root: project.root.clone(),
            writes,
            message: format!("Tidy up {pname}: {}", what.join(", ")),
        });
    }
    if matched != p.drop.len() + p.share.len() + p.on.len() {
        return import::refuse(&headers, STALE);
    }
    if !on_items.is_empty() {
        parts.push(format!(
            "turn {} back on",
            plural(on_items.len(), "skill", "skills")
        ));
    }
    if !drop_homes.is_empty() {
        parts.push(format!(
            "forget {}",
            plural(drop_homes.len(), "dropped copy", "dropped copies")
        ));
    }
    let leave = p.leave.clone();
    if !leave.is_empty() || !on_items.is_empty() || !drop_homes.is_empty() {
        kwrites.push(Write::Placements(Box::new(move |pl| {
            for key in &leave {
                pl.keep.items.insert(key.clone());
            }
            for (home, name, agent) in &on_items {
                let mut placement = pl.of(home, name).clone();
                placement.off.remove(agent);
                pl.set(home, name, placement);
            }
            for (home, name) in &drop_homes {
                pl.remove(home, name);
            }
        })));
    }
    let mut plans = Vec::new();
    if !kwrites.is_empty() {
        plans.push(Plan {
            kind: RepoKind::Knowledge,
            root: s.sources.knowledge.clone(),
            writes: kwrites,
            message: format!("Tidy up: {}", parts.join(", ")),
        });
    }
    plans.extend(plans_after);
    if plans.is_empty() {
        return change::respond(&s, &headers, &change::Outcome::Unchanged, "/skills/tidy");
    }
    let (repos, apply) = import::to_change(plans);
    match change::commit_change(&s, &headers, &p.csrf, repos, apply).await {
        Ok(outcome) => change::respond(&s, &headers, &outcome, "/skills/tidy"),
        Err(e) => e.into_response(&headers),
    }
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_are_read_from_repeated_fields() {
        let p = picked(vec![
            ("csrf".into(), "t".into()),
            ("r0".into(), "bring:skill:~/.claude/skills/a".into()),
            ("r1".into(), "leave:skill:~/.claude/skills/b".into()),
            ("r2".into(), String::new()),
            ("drop".into(), "shop/x".into()),
            ("share".into(), "shop/y".into()),
            ("on".into(), "shop/z/codex".into()),
        ]);
        assert_eq!(p.csrf, "t");
        assert_eq!(p.bring, ["skill:~/.claude/skills/a"]);
        assert_eq!(p.leave, ["skill:~/.claude/skills/b"]);
        assert!(
            p.drop.contains("shop/x")
                && p.share.contains("shop/y")
                && p.on.contains("shop/z/codex")
        );
    }
}
