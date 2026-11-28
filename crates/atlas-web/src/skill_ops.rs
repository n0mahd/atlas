//! Skill library operations: create, delete and move skills between homes. Each call is
//! one change through [`crate::change`] (one commit per repo it touches, however many
//! skills), so it follows the save rules and can be undone with git. The placements file
//! follows a skill when it moves or is deleted.
//!
//! CONTRACT (decision 0027): these signatures are shared by the placement page ("Use
//! in…" moves a skill between Global and a project) and the list page (bulk move and
//! delete). The library worker implements them; callers may rely on them as written.
//!
//! Public skills follow decision 0016: changes to the public repo are committed and never
//! pushed by Atlas (the owner pushes after a privacy check).
//!
//! Private and public skills share one placements namespace (`[global.<name>]`), because
//! atlasd links skills by name. Deleting or moving one global skill drops its placement
//! entry only when no other global home still has a skill of that name.

use atlas_core::catalog::SkillHome;
use atlas_core::kernel::Slug;
use axum::http::HeaderMap;

use crate::AppState;
use crate::change::{ChangeError, Outcome};

/// Creates `<home>/<name>/SKILL.md` with a frontmatter `name` and `description`, on for
/// every agent. Refuses a name that isn't a slug or is taken by any skill folder any agent
/// reads (in the store, a project, or a harness's own folder).
pub(crate) async fn create_skill(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    home: SkillHome,
    name: Slug,
    description: String,
) -> Result<Outcome, ChangeError> {
    let _ = (s, headers, csrf, home, name, description);
    Err(ChangeError::Refused(
        "Creating skills isn't built yet.".to_owned(),
    ))
}

/// Deletes skills' folders and their placement entries, as one change. atlasd removes the
/// links on each machine with the next update.
pub(crate) async fn delete_skills(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    skills: Vec<(SkillHome, Slug)>,
) -> Result<Outcome, ChangeError> {
    let _ = (s, headers, csrf, skills);
    Err(ChangeError::Refused(
        "Deleting skills isn't built yet.".to_owned(),
    ))
}

/// Moves skills' folders to another home (Global ↔ a project, private ↔ public),
/// carrying each placement (per-agent `off` stays; `only-projects` is dropped when it
/// becomes a project skill) and the `skills` CLI's lock record when there is one. Never
/// merges or overwrites: a name already taken at the destination refuses the whole
/// change. Commits the destination repo first, then the source repos.
pub(crate) async fn move_skills(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    skills: Vec<(SkillHome, Slug)>,
    to: SkillHome,
) -> Result<Outcome, ChangeError> {
    let _ = (s, headers, csrf, skills, to);
    Err(ChangeError::Refused(
        "Moving skills isn't built yet.".to_owned(),
    ))
}
