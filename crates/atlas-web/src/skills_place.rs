//! Per-agent switches and "Use in…" for skills: reading what each agent sees on this hub's
//! machine (through `atlas_harness::placement::solve`) and changing a skill's entry in
//! `atlas/placements.toml`.
//!
//! CONTRACT (decision 0027): `set_agents` is shared by the skill's own switches and the
//! list page's bulk and group switches. The placement worker implements it.

use atlas_core::catalog::SkillHome;
use atlas_core::kernel::{HarnessKind, Slug};
use axum::http::HeaderMap;

use crate::AppState;
use crate::change::{ChangeError, Outcome};

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
    let _ = (s, headers, csrf, skills, agent, on);
    Err(ChangeError::Refused(
        "Switching skills isn't built yet.".to_owned(),
    ))
}
