//! Per-agent switches and "Use in…" for skills: reading what each agent sees on this hub's
//! machine (through `atlas_harness::placement::solve`) and changing a skill's entry in
//! `atlas/placements.toml`.
//!
//! CONTRACT (decision 0027): `set_agents` is shared by the skill's own switches and the
//! list page's bulk and group switches. The placement worker implements it.
//!
//! The switches show what the placements file asks for, solved against the agents set up
//! on the hub's machine: an agent that can't be kept away from a skill (it reads the same
//! folder as a wanted agent and has no setting of its own) gets a disabled switch with the
//! reason. Every change goes through [`crate::change`], one commit in the knowledge repo.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use askama::Template;
use atlas_core::catalog::{PLACEMENTS_FILE, Placement, Skill, SkillHome};
use atlas_core::kernel::{HarnessKind, Scope, Slug};
use atlas_harness::placement::{Reach, Solved, solve};
use atlas_harness::switches::can_switch_off;
use atlas_harness::{Layout, layouts};
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use serde::Deserialize;

use crate::change::{self, ChangeError, Outcome};
use crate::edit::wants_json;
use crate::views::{self, Page, Snapshot, join_names, short};
use crate::{AppError, AppState, icons, skill_ops};

/// Turning a skill off for this many agents or more at once asks first.
pub(crate) const ASK_FROM: usize = 2;

// ---------------------------------------------------------------- changing

/// Turns `skills` on or off for `agent`, or for every agent when `agent` is `None`, as
/// one change to the placements file (one commit). Skills already in that state are left
/// as they are.
pub(crate) async fn set_agents(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    skills: Vec<(SkillHome, Slug)>,
    agent: Option<HarnessKind>,
    on: bool,
) -> Result<Outcome, ChangeError> {
    let message = agents_message(&skills, agent, on);
    change::edit_placements(s, headers, csrf, message, move |p| {
        for (home, name) in &skills {
            let mut placement = p.of(home, name).clone();
            match (agent, on) {
                (Some(k), true) => {
                    placement.off.remove(&k);
                }
                (Some(k), false) => {
                    placement.off.insert(k);
                }
                (None, true) => placement.off.clear(),
                (None, false) => placement.off.extend(HarnessKind::agents()),
            }
            p.set(home, name, placement);
        }
        Ok(())
    })
    .await
}

/// Uses a global skill everywhere (`None`) or only in `projects`, as one commit.
pub(crate) async fn set_use(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    home: SkillHome,
    name: Slug,
    projects: Option<BTreeSet<Slug>>,
) -> Result<Outcome, ChangeError> {
    let message = match &projects {
        None => format!("Use {name} globally in Atlas"),
        Some(p) => {
            let names: Vec<&str> = p.iter().map(Slug::as_str).collect();
            format!("Use {name} only in {} in Atlas", and_list(&names))
        }
    };
    change::edit_placements(s, headers, csrf, message, move |p| {
        let mut placement = p.of(&home, &name).clone();
        placement.only_projects = projects;
        p.set(&home, &name, placement);
        Ok(())
    })
    .await
}

fn and_list(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

fn agents_message(skills: &[(SkillHome, Slug)], agent: Option<HarnessKind>, on: bool) -> String {
    let what = match skills {
        [(_, name)] => name.to_string(),
        many => format!("{} skills", many.len()),
    };
    let state = if on { "on" } else { "off" };
    let who = agent.map_or("every agent", short);
    format!("Turn {what} {state} for {who} in Atlas")
}

// ---------------------------------------------------------------- reading

/// How a skill reaches agents, which decides the folders the solver may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Mode {
    /// A global skill used everywhere: links in the login's home folders.
    Home,
    /// A project's own skill: it sits in the repo's `.agents/skills`.
    ProjectOwn,
    /// A global skill used only in some projects: atlasd links it into each checkout.
    ProjectLinked,
}

const PROJECT_OWN: &[&str] = &[".agents/skills"];

impl Mode {
    fn of(home: &SkillHome, placement: &Placement) -> Self {
        match home {
            SkillHome::Project(_) => Self::ProjectOwn,
            _ if placement.global() => Self::Home,
            _ => Self::ProjectLinked,
        }
    }

    const fn reach(self) -> Reach {
        match self {
            Self::Home => Reach::Home,
            Self::ProjectOwn | Self::ProjectLinked => Reach::Project,
        }
    }

    const fn own(self) -> &'static [&'static str] {
        match self {
            Self::ProjectOwn => PROJECT_OWN,
            Self::Home | Self::ProjectLinked => &[],
        }
    }
}

/// The agents set up on the hub's machine and every answer the solver has given for this
/// page, so a page of many skills solves each distinct (wanted agents, mode) once.
struct Solver {
    present: Vec<&'static Layout>,
    solved: BTreeMap<(BTreeSet<HarnessKind>, Mode), Solved>,
}

impl Solver {
    fn new(snap: &Snapshot) -> Self {
        let here: BTreeSet<HarnessKind> = snap
            .harnesses
            .iter()
            .filter(|h| h.present && !h.kind.is_host())
            .map(|h| h.kind)
            .collect();
        Self {
            present: layouts()
                .iter()
                .filter(|l| here.contains(&l.kind))
                .collect(),
            solved: BTreeMap::new(),
        }
    }

    /// Present agents that load skills at this mode's reach, in layout order.
    fn listed(&self, mode: Mode) -> Vec<HarnessKind> {
        self.present
            .iter()
            .filter(|l| !mode.reach().dirs(l).is_empty())
            .map(|l| l.kind)
            .collect()
    }

    fn solve(&mut self, wanted: &BTreeSet<HarnessKind>, mode: Mode) -> Solved {
        let present = &self.present;
        self.solved
            .entry((wanted.clone(), mode))
            .or_insert_with(|| {
                let reach = mode.reach();
                solve(present, wanted, reach, mode.own(), |k| {
                    can_switch_off(k, reach)
                })
            })
            .clone()
    }

    /// Why `kind` sees the skill although it shouldn't: which wanted agents read the
    /// same folder.
    fn why_stuck(
        &self,
        kind: HarnessKind,
        solved: &Solved,
        wanted: &BTreeSet<HarnessKind>,
        mode: Mode,
    ) -> String {
        let reach = mode.reach();
        let reads = |l: &Layout, d: &str| reach.dirs(l).contains(&d);
        let Some(layout) = self.present.iter().find(|l| l.kind == kind) else {
            return String::new();
        };
        let shared: Vec<&str> = solved
            .folders
            .iter()
            .copied()
            .chain(mode.own().iter().copied())
            .filter(|d| reads(layout, d))
            .collect();
        let others: Vec<HarnessKind> = self
            .present
            .iter()
            .filter(|l| l.kind != kind && wanted.contains(&l.kind))
            .filter(|l| shared.iter().any(|d| reads(l, d)))
            .map(|l| l.kind)
            .collect();
        if others.is_empty() {
            format!(
                "{} reads the folder the skill is in and has no setting for this",
                short(kind)
            )
        } else {
            format!(
                "{} reads the same folder as {} and has no setting for this",
                short(kind),
                join_names(&others)
            )
        }
    }
}

/// One agent's switch for one skill.
#[derive(Debug, Clone)]
pub struct Switch {
    pub id: &'static str,
    pub name: &'static str,
    pub icon: &'static str,
    pub on: bool,
    /// Why it can't be turned off here, when it can't.
    pub locked: Option<String>,
    /// Off, but it still sees the skill on this machine, and why.
    pub note: Option<String>,
}

/// A project to tick under "Only these projects".
#[derive(Debug, Clone)]
pub struct ProjectPick {
    pub name: String,
    pub checked: bool,
}

/// Where a switch's form goes back to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Back {
    List,
    Page,
}

impl Back {
    fn parse(s: &str) -> Self {
        if s == "page" { Self::Page } else { Self::List }
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Page => "page",
        }
    }
}

/// Whether the switches work for whoever is looking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Signed in and the placements file is readable.
    Change,
    /// Signed out on a hub with passkeys: offer to sign in.
    SignIn,
    /// A read-only hub, or a placements file that must be fixed first.
    Read,
}

/// Everything the switches and "Use in…" show for one skill.
#[derive(Debug, Clone)]
pub struct SkillPlace {
    pub name: String,
    /// `/skills/<home>/<name>`; the forms post below it.
    pub action: String,
    /// `<home>--<name>`, for element ids (slugs never hold `--`).
    pub dom: String,
    pub all_on: bool,
    /// "On for every agent here", "On for 2 of 3 agents" or "Off for every agent here".
    pub summary: String,
    pub switches: Vec<Switch>,
    /// `project`, `global` or `only`.
    pub use_mode: &'static str,
    /// The project "This project only" means, when the page has one.
    pub current: Option<String>,
    pub projects: Vec<ProjectPick>,
    /// "2 projects" for a global skill used only in some projects.
    pub badge: Option<String>,
    pub back: &'static str,
    /// The scope the page shows, carried back after a change.
    pub scope: String,
    pub access: Access,
    /// Saving sends the change too (the page script asks for the passkey).
    pub send: bool,
    /// Put focus on the switch after a change, so the page returns to this row.
    pub focus: bool,
    /// The placements file can't be read, so nothing can change until it's fixed.
    pub problem: Option<String>,
    /// The sign-in page, coming back to the skill.
    pub signin: String,
}

/// What the page around the switches needs to know.
pub(crate) struct Ctx<'a> {
    pub back: Back,
    pub scope: &'a Scope,
    pub page: &'a Page,
    pub send: bool,
    pub projects: &'a [Slug],
    pub problem: Option<&'a str>,
}

fn place_for(
    solver: &mut Solver,
    skill: &Skill,
    placement: &Placement,
    ctx: &Ctx<'_>,
) -> SkillPlace {
    let mode = Mode::of(&skill.home, placement);
    let listed = solver.listed(mode);
    let wanted: BTreeSet<HarnessKind> = listed
        .iter()
        .copied()
        .filter(|&k| placement.wants(k))
        .collect();
    let now = solver.solve(&wanted, mode);
    let switches: Vec<Switch> = listed
        .iter()
        .map(|&k| {
            let on = wanted.contains(&k);
            let (locked, note) = if on {
                let mut without = wanted.clone();
                without.remove(&k);
                let probe = solver.solve(&without, mode);
                let locked = probe.stuck_on.contains(&k).then(|| {
                    format!(
                        "Can't be turned off here: {}.",
                        solver.why_stuck(k, &probe, &without, mode)
                    )
                });
                (locked, None)
            } else {
                let note = now.stuck_on.contains(&k).then(|| {
                    format!(
                        "Still sees it here: {}.",
                        solver.why_stuck(k, &now, &wanted, mode)
                    )
                });
                (None, note)
            };
            Switch {
                id: k.id(),
                name: short(k),
                icon: icons::icon(k),
                on,
                locked,
                note,
            }
        })
        .collect();
    let on_count = switches.iter().filter(|s| s.on).count();
    let summary = if on_count == switches.len() {
        "On for every agent here".to_owned()
    } else if on_count == 0 {
        "Off for every agent here".to_owned()
    } else {
        format!("On for {on_count} of {} agents", switches.len())
    };
    let only = placement.only_projects.as_ref();
    let current = match &skill.home {
        SkillHome::Project(p) => Some(p.to_string()),
        _ => ctx.scope.project().map(ToString::to_string),
    };
    SkillPlace {
        name: skill.name.to_string(),
        action: format!("/skills/{}/{}", skill.home.key(), skill.name),
        dom: format!("{}--{}", skill.home.key(), skill.name),
        all_on: on_count == switches.len(),
        summary,
        switches,
        use_mode: match (&skill.home, only) {
            (SkillHome::Project(_), _) => "project",
            (_, Some(_)) => "only",
            (_, None) => "global",
        },
        current,
        projects: ctx
            .projects
            .iter()
            .map(|p| ProjectPick {
                name: p.to_string(),
                checked: only.is_some_and(|o| o.contains(p)),
            })
            .collect(),
        badge: only.map(|o| match o.len() {
            0 => "No projects".to_owned(),
            1 => "1 project".to_owned(),
            n => format!("{n} projects"),
        }),
        back: ctx.back.as_str(),
        scope: ctx.page.scope.clone(),
        access: if ctx.problem.is_some() {
            Access::Read
        } else if ctx.page.signed_in {
            Access::Change
        } else if ctx.page.can_sign_in {
            Access::SignIn
        } else {
            Access::Read
        },
        send: ctx.send,
        focus: false,
        problem: ctx.problem.map(ToOwned::to_owned),
        signin: crate::auth::signin_href(&format!("/skills/{}/{}", skill.home.key(), skill.name)),
    }
}

/// Why the switches are off, when the snapshot's placements file can't be read. Pages
/// read placements from the snapshot; only a change reads the file again (under the git
/// lock, in [`change::rewrite_placements`]).
fn placements_problem(snap: &Snapshot) -> Option<String> {
    snap.catalog
        .placements_error
        .as_ref()
        .map(|e| format!("Switches are off until {PLACEMENTS_FILE} is fixed: {e}."))
}

fn send_on_save(s: &AppState) -> bool {
    s.writer.as_ref().is_some_and(|w| w.settings().send_on_save)
}

fn registered(s: &AppState) -> Vec<Slug> {
    s.sources.projects.iter().map(|p| p.name.clone()).collect()
}

impl SkillPlace {
    #[must_use]
    pub fn can_change(&self) -> bool {
        self.access == Access::Change
    }

    #[must_use]
    pub fn can_sign_in(&self) -> bool {
        self.access == Access::SignIn
    }
}

/// The switches for every skill in the store, for the Skills list.
pub struct ListPlace {
    rows: HashMap<String, SkillPlace>,
}

impl ListPlace {
    /// The switches for a row, found by the row's link (`/skills/<home>/<name>`).
    #[must_use]
    pub fn row(&self, href: Option<&str>) -> Option<&SkillPlace> {
        self.rows.get(href?)
    }

    /// Whether the row should open: it is the one just changed.
    #[must_use]
    pub fn is_open(&self, href: Option<&str>) -> bool {
        self.row(href).is_some_and(|p| p.focus)
    }
}

/// Builds the list's switches. `open` is `<home>/<name>` of the row just changed.
pub(crate) fn list(
    s: &AppState,
    snap: &Snapshot,
    scope: &Scope,
    page: &Page,
    open: &str,
) -> ListPlace {
    let placements = &snap.catalog.placements;
    let problem = placements_problem(snap);
    let projects = registered(s);
    let ctx = Ctx {
        back: Back::List,
        scope,
        page,
        send: send_on_save(s),
        projects: &projects,
        problem: problem.as_deref(),
    };
    let mut solver = Solver::new(snap);
    let rows = snap
        .catalog
        .skills
        .iter()
        .map(|skill| {
            let mut place = place_for(
                &mut solver,
                skill,
                placements.of(&skill.home, &skill.name),
                &ctx,
            );
            place.focus = !open.is_empty() && place.action == format!("/skills/{open}");
            (place.action.clone(), place)
        })
        .collect();
    ListPlace { rows }
}

/// The switches on a skill's own page.
pub(crate) fn detail(
    s: &AppState,
    snap: &Snapshot,
    skill: &Skill,
    scope: &Scope,
    page: &Page,
) -> SkillPlace {
    let placements = &snap.catalog.placements;
    let problem = placements_problem(snap);
    let projects = registered(s);
    let ctx = Ctx {
        back: Back::Page,
        scope,
        page,
        send: send_on_save(s),
        projects: &projects,
        problem: problem.as_deref(),
    };
    let mut solver = Solver::new(snap);
    place_for(
        &mut solver,
        skill,
        placements.of(&skill.home, &skill.name),
        &ctx,
    )
}

/// The flash line after a change, from the redirect's own validated parameters (never
/// free text from the address).
#[must_use]
pub fn did_text(turned: &str, agent: &str, used: &str) -> Option<String> {
    match (turned, used) {
        ("on" | "off", _) => {
            let who = if agent == "all" {
                "every agent".to_owned()
            } else {
                let k: HarnessKind = agent.parse().ok().filter(|k: &HarnessKind| !k.is_host())?;
                short(k).to_owned()
            };
            Some(format!("Turned {turned} for {who}"))
        }
        (_, "global") => Some("Now used globally".to_owned()),
        (_, "only") => Some("Now used only in the projects you chose".to_owned()),
        (_, "project") => Some("Moved into the project".to_owned()),
        _ => None,
    }
}

// ---------------------------------------------------------------- pages

/// A step between a form and its change: "turn it off for every agent?" or why a change
/// was refused.
#[derive(Template)]
#[template(path = "skill_place_step.html")]
struct StepPage {
    page: Page,
    ask: Option<Ask>,
    refused: String,
    back_href: String,
}

struct Ask {
    text: String,
    still: String,
    action: String,
    back: &'static str,
    scope: String,
    send: bool,
}

fn find<'a>(snap: &'a Snapshot, home: &str, name: &str) -> Option<&'a Skill> {
    snap.catalog
        .skills
        .iter()
        .find(|s| s.home.key() == home && s.name.as_str() == name)
}

/// Where to go after a change: the skill's page, or the list with its row open. Built
/// only from validated values.
fn next_href(back: Back, home: &SkillHome, name: &Slug, scope: &Scope, did: &str) -> String {
    let scope_key = scope.project().map_or("global", Slug::as_str);
    match back {
        Back::Page => format!("/skills/{}/{name}?scope={scope_key}&{did}", home.key()),
        Back::List => format!("/skills?scope={scope_key}&open={}/{name}&{did}", home.key()),
    }
}

fn back_href(back: Back, skill: &Skill, scope: &Scope) -> String {
    let scope_key = scope.project().map_or("global", Slug::as_str);
    match back {
        Back::Page => format!(
            "/skills/{}/{}?scope={scope_key}",
            skill.home.key(),
            skill.name
        ),
        Back::List => format!(
            "/skills?scope={scope_key}&open={}/{}",
            skill.home.key(),
            skill.name
        ),
    }
}

async fn step(
    s: &AppState,
    headers: &HeaderMap,
    status: StatusCode,
    title: String,
    ask: Option<Ask>,
    refused: String,
    back_href: String,
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let page = s.page(headers, "/skills", title, &snap, &Scope::Global, false);
    Ok((
        status,
        Html(
            StepPage {
                page,
                ask,
                refused,
                back_href,
            }
            .render()?,
        ),
    )
        .into_response())
}

async fn finish(
    s: &AppState,
    headers: &HeaderMap,
    result: Result<Outcome, ChangeError>,
    next: &str,
    back: String,
) -> Result<Response, AppError> {
    match result {
        Ok(outcome) => Ok(change::respond(s, headers, &outcome, next)),
        // The page script shows a refusal in place; a page here would make it post again.
        Err(e @ (ChangeError::Forbidden | ChangeError::ReadOnly)) => Ok(e.into_response(headers)),
        Err(e @ ChangeError::Refused(_)) if wants_json(headers) => Ok(e.into_response(headers)),
        Err(ChangeError::Refused(msg)) => {
            step(
                s,
                headers,
                StatusCode::CONFLICT,
                "Nothing changed".to_owned(),
                None,
                msg,
                back,
            )
            .await
        }
    }
}

#[derive(Deserialize)]
pub struct AgentsForm {
    #[serde(default)]
    csrf: String,
    #[serde(default)]
    agent: String,
    #[serde(default)]
    on: String,
    #[serde(default)]
    confirmed: String,
    #[serde(default)]
    back: String,
    #[serde(default)]
    scope: String,
}

fn bad(msg: &str) -> AppError {
    AppError::Bad(msg.to_owned())
}

/// `POST /skills/{home}/{name}/agents`: one agent's switch (`agent=<id>`) or the switch
/// for every agent (`agent=all`), `on=1` or `on=0`.
pub(crate) async fn post_agents(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name)): Path<(String, String)>,
    Form(f): Form<AgentsForm>,
) -> Response {
    if let Err(e) = change::authorise(&s, &headers, &f.csrf) {
        return e.into_response(&headers);
    }
    agents(&s, &headers, &home, &name, f)
        .await
        .unwrap_or_else(|e| change::form_error(&headers, e))
}

async fn agents(
    s: &AppState,
    headers: &HeaderMap,
    home: &str,
    name: &str,
    f: AgentsForm,
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let skill = find(&snap, home, name).ok_or(AppError::NotFound)?;
    let agent = match f.agent.as_str() {
        "all" => None,
        id => Some(
            id.parse::<HarnessKind>()
                .ok()
                .filter(|k| !k.is_host())
                .ok_or_else(|| bad("That isn't an agent Atlas knows. Nothing changed."))?,
        ),
    };
    let on = match f.on.as_str() {
        "1" => true,
        "0" => false,
        _ => return Err(bad("Say on or off. Nothing changed.")),
    };
    let scope = views::resolve_scope(&snap, &f.scope);
    let back = Back::parse(&f.back);
    let back_to = back_href(back, skill, &scope);

    if !on && agent.is_none() && f.confirmed != "1" {
        let placement = snap.catalog.placements.of(&skill.home, &skill.name);
        let mode = Mode::of(&skill.home, placement);
        let mut solver = Solver::new(&snap);
        let turning: Vec<HarnessKind> = solver
            .listed(mode)
            .into_iter()
            .filter(|&k| placement.wants(k))
            .collect();
        if turning.len() >= ASK_FROM {
            let none = solver.solve(&BTreeSet::new(), mode);
            let seeing: Vec<HarnessKind> = none.stuck_on.iter().copied().collect();
            let ask = Ask {
                text: format!(
                    "This turns {} off for {}. Git keeps the change, so it can be undone.",
                    skill.name,
                    join_names(&turning)
                ),
                still: if seeing.is_empty() {
                    String::new()
                } else {
                    format!(
                        "{} will still see it here: no setting of theirs can turn it off.",
                        join_names(&seeing)
                    )
                },
                action: format!("/skills/{}/{}/agents", skill.home.key(), skill.name),
                back: back.as_str(),
                scope: scope
                    .project()
                    .map_or_else(|| "global".to_owned(), ToString::to_string),
                send: send_on_save(s),
            };
            return step(
                s,
                headers,
                StatusCode::OK,
                format!("Turn {} off?", skill.name),
                Some(ask),
                String::new(),
                back_to,
            )
            .await;
        }
    }

    let did = format!(
        "turned={}&agent={}",
        if on { "on" } else { "off" },
        agent.map_or("all", HarnessKind::id)
    );
    let next = next_href(back, &skill.home, &skill.name, &scope, &did);
    let result = set_agents(
        s,
        headers,
        &f.csrf,
        vec![(skill.home.clone(), skill.name.clone())],
        agent,
        on,
    )
    .await;
    finish(s, headers, result, &next, back_to).await
}

/// Where "Use in…" puts a skill.
enum UseChoice {
    /// Moved into this project's `.agents/skills`.
    Project(Slug),
    /// Global, in the home folders.
    Global,
    /// One global copy, linked into these projects only.
    Only(BTreeSet<Slug>),
}

impl UseChoice {
    const fn word(&self) -> &'static str {
        match self {
            Self::Project(_) => "project",
            Self::Global => "global",
            Self::Only(_) => "only",
        }
    }
}

/// Carries out a "Use in…" choice. Returns the outcome and the skill's home afterwards.
async fn change_use(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    from: SkillHome,
    name: Slug,
    choice: UseChoice,
) -> (Result<Outcome, ChangeError>, SkillHome) {
    let in_project = matches!(from, SkillHome::Project(_));
    match choice {
        UseChoice::Project(p) => {
            let to = SkillHome::Project(p);
            if from == to {
                return (Ok(Outcome::Unchanged), to);
            }
            let result =
                skill_ops::move_skills(s, headers, csrf, vec![(from, name)], to.clone()).await;
            (result, to)
        }
        UseChoice::Global if in_project => {
            let to = SkillHome::Private;
            let result =
                skill_ops::move_skills(s, headers, csrf, vec![(from, name)], to.clone()).await;
            (result, to)
        }
        UseChoice::Global => {
            let result = set_use(s, headers, csrf, from.clone(), name, None).await;
            (result, from)
        }
        UseChoice::Only(chosen) if in_project => {
            // A project's skill becomes a global one used only in `chosen`, in one change.
            let to = SkillHome::Private;
            let result =
                skill_ops::move_to_global_only_in(s, headers, csrf, from, name, chosen).await;
            (result, to)
        }
        UseChoice::Only(chosen) => {
            let result = set_use(s, headers, csrf, from.clone(), name, Some(chosen)).await;
            (result, from)
        }
    }
}

/// `POST /skills/{home}/{name}/use`: `use=project` (with `project`), `use=global`, or
/// `use=only` (with one `projects` field per project).
pub(crate) async fn post_use(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path((home, name)): Path<(String, String)>,
    Form(fields): Form<Vec<(String, String)>>,
) -> Response {
    let csrf = fields
        .iter()
        .find(|(k, _)| k == "csrf")
        .map_or("", |(_, v)| v.as_str());
    if let Err(e) = change::authorise(&s, &headers, csrf) {
        return e.into_response(&headers);
    }
    use_in(&s, &headers, &home, &name, &fields)
        .await
        .unwrap_or_else(|e| change::form_error(&headers, e))
}

async fn use_in(
    s: &AppState,
    headers: &HeaderMap,
    home: &str,
    name: &str,
    fields: &[(String, String)],
) -> Result<Response, AppError> {
    let snap = s.snapshot().await?;
    let skill = find(&snap, home, name).ok_or(AppError::NotFound)?;
    let field = |key: &str| {
        fields
            .iter()
            .find(|(k, _)| k == key)
            .map_or("", |(_, v)| v.as_str())
    };
    let projects = registered(s);
    let project = |value: &str| -> Result<Slug, AppError> {
        projects
            .iter()
            .find(|p| p.as_str() == value)
            .cloned()
            .ok_or_else(|| bad("That isn't a project Atlas knows. Nothing changed."))
    };
    let choice = match field("use") {
        "project" => UseChoice::Project(project(field("project"))?),
        "global" => UseChoice::Global,
        "only" => {
            let chosen = fields
                .iter()
                .filter(|(k, _)| k == "projects")
                .map(|(_, v)| project(v))
                .collect::<Result<BTreeSet<Slug>, AppError>>()?;
            if chosen.is_empty() {
                return Err(bad("Pick at least one project. Nothing changed."));
            }
            UseChoice::Only(chosen)
        }
        _ => return Err(bad("Choose where to use it. Nothing changed.")),
    };
    let scope = views::resolve_scope(&snap, field("scope"));
    let back = Back::parse(field("back"));
    let back_to = back_href(back, skill, &scope);
    // After moving into a project, the list shows that project.
    let to_scope = match &choice {
        UseChoice::Project(p) => Scope::Project(p.clone()),
        _ => scope,
    };
    let used = format!("used={}", choice.word());
    let (result, to_home) = change_use(
        s,
        headers,
        field("csrf"),
        skill.home.clone(),
        skill.name.clone(),
        choice,
    )
    .await;
    let next = next_href(back, &to_home, &skill.name, &to_scope, &used);
    finish(s, headers, result, &next, back_to).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_read_as_english() {
        let one = vec![(SkillHome::Private, Slug::new("pdf-tools").unwrap())];
        assert_eq!(
            agents_message(&one, Some(HarnessKind::Codex), false),
            "Turn pdf-tools off for Codex in Atlas"
        );
        let two = vec![one[0].clone(), (SkillHome::Public, Slug::new("x").unwrap())];
        assert_eq!(
            agents_message(&two, None, true),
            "Turn 2 skills on for every agent in Atlas"
        );
    }

    #[test]
    fn flash_text_comes_only_from_known_values() {
        assert_eq!(
            did_text("off", "codex", "").as_deref(),
            Some("Turned off for Codex")
        );
        assert_eq!(
            did_text("on", "all", "").as_deref(),
            Some("Turned on for every agent")
        );
        assert_eq!(did_text("off", "<script>", ""), None);
        assert_eq!(did_text("off", "t3", ""), None);
        assert_eq!(did_text("maybe", "codex", ""), None);
        assert_eq!(
            did_text("", "", "global").as_deref(),
            Some("Now used globally")
        );
    }

    fn snap_with(kinds: &[HarnessKind]) -> Snapshot {
        use atlas_core::catalog::Catalog;
        use atlas_core::distribution::HarnessView;
        Snapshot {
            catalog: Catalog::default(),
            harnesses: kinds
                .iter()
                .map(|&kind| HarnessView {
                    kind,
                    present: true,
                    skill_dirs: Vec::new(),
                    instructions: Vec::new(),
                    hook_config: None,
                    hooks: Vec::new(),
                })
                .collect(),
            projects: Vec::new(),
            library: crate::library::Index::default(),
            outside: atlas_fs::outside::Outside::default(),
        }
    }

    #[test]
    fn a_page_of_default_skills_solves_once_per_wanted_set() {
        let snap = snap_with(&HarnessKind::agents().collect::<Vec<_>>());
        let mut solver = Solver::new(&snap);
        let listed = solver.listed(Mode::Home);
        let all: BTreeSet<HarnessKind> = listed.iter().copied().collect();
        for _ in 0..100 {
            solver.solve(&all, Mode::Home);
            for k in &listed {
                let mut w = all.clone();
                w.remove(k);
                solver.solve(&w, Mode::Home);
            }
        }
        assert_eq!(solver.solved.len(), 1 + listed.len());
    }

    #[test]
    fn an_agent_sharing_a_folder_without_a_setting_is_locked_with_the_reason() {
        use HarnessKind::{Codex, Grok};
        let snap = snap_with(&[Codex, Grok]);
        let mut solver = Solver::new(&snap);
        let wanted = BTreeSet::from([Codex]);
        let probe = solver.solve(&wanted, Mode::ProjectOwn);
        assert!(probe.stuck_on.contains(&Grok));
        assert_eq!(
            solver.why_stuck(Grok, &probe, &wanted, Mode::ProjectOwn),
            "Grok reads the same folder as Codex and has no setting for this"
        );
    }
}
