//! Skill library operations: create, delete and move a skill between homes. Each is one
//! change through [`crate::change`], so it follows the save rules and can be undone with
//! git. The placements file follows a skill when it moves or is deleted.
//!
//! CONTRACT (decision 0027): these signatures are shared by the placement page ("Use
//! in…" moves a skill between Global and a project) and the list page (bulk move and
//! delete). The library worker implements them; callers may rely on them as written.

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

/// Deletes a skill's folder and its placement entry. atlasd removes the links on each
/// machine with the next update. Public skills are refused (they're deleted by hand
/// after a privacy check, like every public change).
pub(crate) async fn delete_skill(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    home: SkillHome,
    name: Slug,
) -> Result<Outcome, ChangeError> {
    let _ = (s, headers, csrf, home, name);
    Err(ChangeError::Refused(
        "Deleting skills isn't built yet.".to_owned(),
    ))
}

/// Moves a skill's folder to another home (Global ↔ a project, private ↔ project),
/// carrying its placement (per-agent `off` stays; `only-projects` is dropped when it
/// becomes a project skill) and the `skills` CLI's lock record when there is one. Never
/// merges or overwrites: a name already taken at the destination is refused. Commits the
/// destination repo first, then the source.
pub(crate) async fn move_skill(
    s: &AppState,
    headers: &HeaderMap,
    csrf: &str,
    from: SkillHome,
    name: Slug,
    to: SkillHome,
) -> Result<Outcome, ChangeError> {
    let _ = (s, headers, csrf, from, name, to);
    Err(ChangeError::Refused(
        "Moving skills isn't built yet.".to_owned(),
    ))
}
