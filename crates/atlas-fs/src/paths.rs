use std::path::{Path, PathBuf};

use atlas_core::catalog::Problem;
use atlas_core::kernel::Slug;

/// A registered project: a git repo whose skills and instructions Atlas reads.
#[derive(Clone, Debug)]
pub struct Project {
    pub name: Slug,
    pub root: PathBuf,
}

/// Where the store lives on the hub.
#[derive(Clone, Debug)]
pub struct Sources {
    /// The login's home folder, used to show paths as `~/...`.
    pub home: PathBuf,
    /// The private knowledge repo: `AGENTS.md`, `skills/`, `memory/`, `hooks/`.
    pub knowledge: PathBuf,
    /// The public skills repo, if there is one.
    pub public_skills: Option<PathBuf>,
    pub projects: Vec<Project>,
}

impl Sources {
    /// Shows a path with the home folder as `~`.
    #[must_use]
    pub fn display(&self, path: &Path) -> String {
        match path.strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        }
    }
}

/// Finds projects: direct children of `dir` that are git repos with an `AGENTS.md`,
/// `CLAUDE.md` or `.agents/` folder. Names that aren't valid slugs come back as problems.
#[must_use]
pub fn discover_projects(dir: &Path) -> (Vec<Project>, Vec<Problem>) {
    let mut projects = Vec::new();
    let mut problems = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (projects, problems);
    };
    for entry in entries.flatten() {
        let root = entry.path();
        let is_dir = entry.file_type().is_ok_and(|t| t.is_dir());
        if !is_dir || !root.join(".git").exists() {
            continue;
        }
        let has_agent_files = ["AGENTS.md", "CLAUDE.md", ".agents"]
            .iter()
            .any(|f| root.join(f).exists());
        if !has_agent_files {
            continue;
        }
        let file_name = entry.file_name().to_string_lossy().into_owned();
        match Slug::new(file_name.clone()) {
            Ok(name) => projects.push(Project { name, root }),
            Err(e) => problems.push(Problem {
                path: root.display().to_string(),
                message: format!("project folder skipped: {e}"),
            }),
        }
    }
    projects.sort_by(|a, b| a.name.cmp(&b.name));
    (projects, problems)
}
