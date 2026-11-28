//! A harness's global instruction file: a link to the store's `AGENTS.md`, or, for
//! Claude Code's `CLAUDE.md`, an `@<path>` import line. Atlas never overwrites a file
//! with content of its own, except one whose every non-blank line is already in the
//! store's file ([`contained`]), so nothing is lost.
//!
//! Also the pure text rules the hub uses to move instruction text between files
//! ([`append_section`]), Claude Code's AGENTS.md setting ([`claude_agents_md_setting`]) and
//! Atlas's block in a repo's `.git/info/exclude` ([`exclude_with`]).

use std::path::{Path, PathBuf};

use atlas_core::catalog::ClaudeAgentsMd;
use serde_json::{Map, Value};

use crate::change::shape;
use crate::json::{finish, parse_object};
use crate::skills::normalize;
use crate::{Change, FormatError};

/// What sits at the harness's instruction file path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionFile {
    Missing,
    /// A symlink, with its target as read (relative targets resolve against the
    /// file's folder).
    Link(PathBuf),
    /// A regular file and its contents.
    File(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionAction {
    /// Already right.
    Keep,
    CreateLink {
        target: PathBuf,
    },
    /// The link points at an older Atlas path.
    ReplaceLink {
        from: PathBuf,
        to: PathBuf,
    },
    /// Claude Code only: write these contents (the file with the import appended, or,
    /// when all its own text is already in the store's file, just the import).
    Write(Vec<u8>),
    /// The agent's own file holds nothing the store's file doesn't: replace it with a link.
    ReplaceFileWithLink {
        target: PathBuf,
    },
    Conflict(InstructionConflict),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstructionConflict {
    /// A regular file with text that isn't in the store's file is in the way.
    UserFile,
    /// A link points somewhere Atlas doesn't manage.
    ForeignLink { target: PathBuf },
    /// Claude Code would read the import as several words.
    UnimportablePath,
}

/// For harnesses that read `AGENTS.md` directly (Codex, `OpenCode`, Pi, Antigravity):
/// the instruction file should be a link to `store`.
///
/// `file` is the instruction file's own path, used to resolve relative link targets.
/// A link into one of `managed_roots` is Atlas's and may be repointed. A regular file is
/// replaced by the link only when every non-blank line of it is in `store_text` (the
/// store file's text); otherwise it is the user's and reported.
#[must_use]
pub fn plan_instruction_link(
    file: &Path,
    current: &InstructionFile,
    store: &Path,
    store_text: &str,
    managed_roots: &[PathBuf],
) -> InstructionAction {
    let store = normalize(store);
    match current {
        InstructionFile::Missing => InstructionAction::CreateLink { target: store },
        InstructionFile::File(text) => {
            if contained(text, store_text, |_| false) {
                InstructionAction::ReplaceFileWithLink { target: store }
            } else {
                InstructionAction::Conflict(InstructionConflict::UserFile)
            }
        }
        InstructionFile::Link(t) => {
            let target = resolve(file, t);
            if target == store {
                InstructionAction::Keep
            } else if managed_roots
                .iter()
                .any(|r| target.starts_with(normalize(r)))
            {
                InstructionAction::ReplaceLink {
                    from: target,
                    to: store,
                }
            } else {
                InstructionAction::Conflict(InstructionConflict::ForeignLink { target })
            }
        }
    }
}

/// The import line Claude Code reads for `store`.
#[must_use]
pub fn claude_import(store: &Path) -> String {
    format!("@{}", store.display())
}

/// For Claude Code's `~/.claude/CLAUDE.md`: the file should hold one `@<store>` import
/// line. A missing file is created with just that line. In an existing file, an import
/// that points into one of `managed_roots` (an older Atlas path, or a folder Atlas has
/// adopted from earlier scripts, such as `@~/Knowledge/AGENTS.md`) is replaced in place
/// rather than duplicated, so Claude Code doesn't load the instructions twice; otherwise
/// the line is appended. Nothing else changes, unless every other non-blank line is
/// already in `store_text` (the store file's text, after "Use Global instead"): then the
/// file becomes just the import, so Claude doesn't read that text twice. A link is left
/// alone unless it already points at `store`, because writing would go through it.
/// `home` expands `@~/` imports.
#[must_use]
pub fn plan_claude_md(
    file: &Path,
    current: &InstructionFile,
    store: &Path,
    store_text: &str,
    managed_roots: &[PathBuf],
    home: &Path,
) -> InstructionAction {
    let store = normalize(store);
    let import = claude_import(&store);
    if import.chars().any(char::is_whitespace) {
        return InstructionAction::Conflict(InstructionConflict::UnimportablePath);
    }
    match current {
        InstructionFile::Missing => InstructionAction::Write(format!("{import}\n").into_bytes()),
        InstructionFile::Link(t) => {
            let target = resolve(file, t);
            if target == store {
                InstructionAction::Keep
            } else {
                InstructionAction::Conflict(InstructionConflict::ForeignLink { target })
            }
        }
        InstructionFile::File(text) => {
            let dir = file.parent().unwrap_or(Path::new("/"));
            let imported = |line: &str| imports_path(line, dir, home);
            let managed = |p: &Path| managed_roots.iter().any(|r| p.starts_with(normalize(r)));
            let is_atlas_import =
                |line: &str| is_atlas_import(line, &store, managed_roots, dir, home);
            let has_own = text
                .lines()
                .any(|l| !l.trim().is_empty() && !is_atlas_import(l));
            if has_own && contained(text, store_text, is_atlas_import) {
                return InstructionAction::Write(format!("{import}\n").into_bytes());
            }
            let mut placed = false;
            let mut lines: Vec<String> = Vec::new();
            for line in text.lines() {
                match imported(line) {
                    Some(p) if p == store || managed(&p) => {
                        // One Atlas import, where the first one was: a line already
                        // importing the store stays as written; an older managed path
                        // becomes the store's import; repeats go.
                        if !placed {
                            lines.push(if p == store {
                                line.to_owned()
                            } else {
                                import.clone()
                            });
                            placed = true;
                        }
                    }
                    _ => lines.push(line.to_owned()),
                }
            }
            if !placed {
                lines.push(import.clone());
            }
            let mut out = lines.join("\n");
            out.push('\n');
            if out == *text || (!text.ends_with('\n') && out.trim_end_matches('\n') == text) {
                return InstructionAction::Keep;
            }
            InstructionAction::Write(out.into_bytes())
        }
    }
}

fn resolve(file: &Path, target: &Path) -> PathBuf {
    normalize(&file.parent().unwrap_or(Path::new("/")).join(target))
}

/// A line as compared: without its line ending (`\r` included) and trailing spaces.
fn line_key(line: &str) -> &str {
    line.trim_end()
}

/// Whether every non-blank line of `text` (except those `skip` says to ignore) is a line of
/// `global`. Exact per line, apart from trailing whitespace, so a file this says yes to
/// can be replaced by a link or import of `global` without losing anything it says.
#[must_use]
pub fn contained(text: &str, global: &str, skip: impl Fn(&str) -> bool) -> bool {
    let have: std::collections::HashSet<&str> = global.lines().map(line_key).collect();
    text.lines()
        .filter(|l| !l.trim().is_empty() && !skip(l))
        .all(|l| have.contains(line_key(l)))
}

/// The marker line [`append_section`] puts before text it adds: an HTML comment, so it
/// doesn't show when the file is rendered, naming where the text came from.
#[must_use]
pub fn section_marker(from: &str) -> String {
    let from = from.replace("--", "-").replace(['\n', '\r'], " ");
    format!("<!-- atlas: from {from} -->")
}

/// `into` with `text` added at its end, for moving instructions between files: a blank
/// line, [`section_marker`]`(from)`, a blank line, then `text` without its leading and
/// trailing blank lines. Nothing is added when `text` is blank or every non-blank line of
/// it is in `into` already ([`contained`]), so running it twice changes nothing.
/// Deterministic: the same inputs always give the same bytes.
#[must_use]
pub fn append_section(into: &str, from: &str, text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let start = lines.iter().position(|l| !l.trim().is_empty());
    let end = lines.iter().rposition(|l| !l.trim().is_empty());
    let (Some(start), Some(end)) = (start, end) else {
        return into.to_owned();
    };
    if contained(text, into, |_| false) {
        return into.to_owned();
    }
    let body = lines[start..=end].join("\n");
    let mut out = String::new();
    if !into.trim().is_empty() {
        out.push_str(into);
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(&section_marker(from));
    out.push_str("\n\n");
    out.push_str(&body);
    out.push('\n');
    out
}

/// `text` without its lines that import `target` with an `@` line by themselves. `dir` is
/// the importing file's folder (relative imports resolve there), `home` expands `@~/`.
#[must_use]
pub fn without_imports_of(text: &str, target: &Path, dir: &Path, home: &Path) -> String {
    let target = normalize(target);
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| imports_path(l, dir, home).is_none_or(|p| p != target))
        .collect();
    let mut out = kept.join("\n");
    if !out.is_empty() && text.ends_with('\n') {
        out.push('\n');
    }
    out
}

/// Whether `line` is Atlas's own import of the global instructions: an `@<path>` line by
/// itself importing `store`, or an older Atlas path in one of `managed_roots` (such as an
/// adopted `@~/Knowledge/AGENTS.md`). Such a line is Atlas's to replace, so it never
/// counts as the owner's text that must be kept. `dir` is the importing file's folder.
#[must_use]
pub fn is_atlas_import(
    line: &str,
    store: &Path,
    managed_roots: &[PathBuf],
    dir: &Path,
    home: &Path,
) -> bool {
    let store = normalize(store);
    imports_path(line, dir, home)
        .is_some_and(|p| p == store || managed_roots.iter().any(|r| p.starts_with(normalize(r))))
}

/// The path a line imports, if it is an `@<path>` line by itself.
#[must_use]
pub fn imports_path(line: &str, dir: &Path, home: &Path) -> Option<PathBuf> {
    let path = line.trim().strip_prefix('@')?;
    if path.is_empty() || path.chars().any(char::is_whitespace) {
        return None;
    }
    let path = match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => dir.join(path),
    };
    Some(normalize(&path))
}

// ---------------------------------------------------------------- Claude's AGENTS.md setting

/// The plugin entry in `~/.claude/settings.json` that holds Claude Code's AGENTS.md
/// option (Claude Code 2.1.277 and later).
pub const CLAUDE_AGENTS_MD_PLUGIN: &str = "cc-plugin-agents-md@builtin";
/// Its id before Claude Code 2.1.285, which later versions still read.
pub const CLAUDE_AGENTS_MD_LEGACY_PLUGIN: &str = "agents-md@builtin";
/// The option's key under `pluginConfigs.<plugin>.options`.
pub const CLAUDE_AGENTS_MD_OPTION: &str = "instructionFiles";

/// Claude Code's own word for a choice.
#[must_use]
pub const fn claude_value(choice: ClaudeAgentsMd) -> &'static str {
    match choice {
        ClaudeAgentsMd::WhenNoClaudeMd => "claude-md-or-agents-md",
        ClaudeAgentsMd::Alongside => "claude-md-and-agents-md",
        ClaudeAgentsMd::Never => "claude-md",
    }
}

/// What Claude Code's setting says in `settings.json`, as Claude Code's own word
/// (`managed-only` included), or `None` when it isn't set or the file can't be read.
#[must_use]
pub fn read_claude_agents_md(current: Option<&[u8]>) -> Option<String> {
    let map = parse_object(current).ok()?;
    [CLAUDE_AGENTS_MD_PLUGIN, CLAUDE_AGENTS_MD_LEGACY_PLUGIN]
        .iter()
        .find_map(|id| {
            map.get("pluginConfigs")?
                .get(id)?
                .get("options")?
                .get(CLAUDE_AGENTS_MD_OPTION)?
                .as_str()
                .map(str::to_owned)
        })
}

/// The choice a Claude Code word stands for; `None` for `managed-only` and unknown words.
#[must_use]
pub fn claude_choice(value: &str) -> Option<ClaudeAgentsMd> {
    ClaudeAgentsMd::ALL
        .into_iter()
        .find(|c| claude_value(*c) == value)
}

/// Sets Claude Code's AGENTS.md option in `settings.json` to `choice`:
/// `pluginConfigs["cc-plugin-agents-md@builtin"].options.instructionFiles`, and the legacy
/// `agents-md@builtin` entry too when it already has a value, so the two can't disagree.
/// Every other key (Atlas's hooks, skill switches, the owner's settings) stays as it is,
/// so this composes with the other formats that write the same file: pass it their
/// output as `current`.
///
/// # Errors
/// The file isn't JSON, or `pluginConfigs`, the plugin's entry or its `options` exist but
/// aren't objects; Atlas then leaves the file alone.
pub fn claude_agents_md_setting(
    current: Option<&[u8]>,
    choice: ClaudeAgentsMd,
) -> Result<Change, FormatError> {
    let original = parse_object(current)?;
    let mut edited = original.clone();
    let value = Value::from(claude_value(choice));
    let configs = object_at(&mut edited, "pluginConfigs", "pluginConfigs")?;
    let legacy_set = configs
        .get(CLAUDE_AGENTS_MD_LEGACY_PLUGIN)
        .and_then(|p| p.get("options"))
        .and_then(|o| o.get(CLAUDE_AGENTS_MD_OPTION))
        .is_some();
    let mut ids = vec![CLAUDE_AGENTS_MD_PLUGIN];
    if legacy_set {
        ids.push(CLAUDE_AGENTS_MD_LEGACY_PLUGIN);
    }
    for id in ids {
        let at = format!("pluginConfigs.{id}");
        let plugin = object_at(configs, id, &at)?;
        let options = object_at(plugin, "options", &format!("{at}.options"))?;
        options.insert(CLAUDE_AGENTS_MD_OPTION.to_owned(), value.clone());
    }
    finish(&original, &edited)
}

/// The object under `key`, created when missing.
fn object_at<'a>(
    map: &'a mut Map<String, Value>,
    key: &str,
    at: &str,
) -> Result<&'a mut Map<String, Value>, FormatError> {
    map.entry(key.to_owned())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| shape(at, "an object"))
}

// ---------------------------------------------------------------- .git/info/exclude

/// The lines that open and close Atlas's block in a repo's `.git/info/exclude`.
pub const EXCLUDE_BEGIN: &str = "# atlas:begin";
pub const EXCLUDE_END: &str = "# atlas:end";

/// `current` (a `.git/info/exclude`) with `add` (patterns such as `/CLAUDE.local.md`) in
/// Atlas's block, which is created at the end when missing. Lines outside the block are
/// never touched; entries already in the block stay, in their order, and new ones follow.
/// Shared by every Atlas entry in the file.
///
/// A block whose closing line is gone (a hand edit) runs to the end of the file and is
/// closed there, so there is only ever one. With nothing to add and no block, `current`
/// comes back as it is.
#[must_use]
pub fn exclude_with(current: &str, add: &[&str]) -> String {
    let lines: Vec<&str> = current.lines().collect();
    let Some(begin) = lines.iter().position(|l| l.trim_end() == EXCLUDE_BEGIN) else {
        if add.is_empty() {
            return current.to_owned();
        }
        return render_exclude(&lines, Vec::new(), add, &[]);
    };
    let end = lines[begin..]
        .iter()
        .position(|l| l.trim_end() == EXCLUDE_END)
        .map_or(lines.len(), |e| begin + e);
    let after = lines.get(end + 1..).unwrap_or_default();
    render_exclude(&lines[..begin], lines[begin + 1..end].to_vec(), add, after)
}

fn render_exclude<'a>(
    before: &[&'a str],
    mut block: Vec<&'a str>,
    add: &[&'a str],
    after: &[&'a str],
) -> String {
    for a in add {
        if !block.iter().any(|l| l.trim_end() == *a) {
            block.push(a);
        }
    }
    let mut out: Vec<&str> = before.to_vec();
    out.push(EXCLUDE_BEGIN);
    out.extend(block);
    out.push(EXCLUDE_END);
    out.extend(after);
    let mut text = out.join("\n");
    text.push('\n');
    text
}
