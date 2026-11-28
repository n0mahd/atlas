//! The docs are the map agents read before changing anything, so they must describe the
//! code. This test fails when `documents/architecture.md` and the workspace disagree
//! about which crates exist, when a living doc names a repo path that isn't there, or
//! when `documents/strategy.md` loses a section.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Docs whose paths are checked. Decision records are history and may name things that
/// have since moved.
const LIVING_DOCS: &[&str] = &[
    "AGENTS.md",
    "README.md",
    "documents/architecture.md",
    "documents/strategy.md",
];

/// A backticked token is treated as a repo path when it starts with one of these.
const ROOTS: &[&str] = &[
    "crates/",
    "deploy/",
    "documents/",
    ".github/",
    "site/",
    "assets/",
    "e2e/",
];

const STRATEGY_SECTIONS: &[&str] = &[
    "## Purpose",
    "## Priorities",
    "## Invariants",
    "## Direction",
    "## Debt register",
    "## Open decisions",
    "## Keeping this true",
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn read(rel: &str) -> String {
    fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// The text between backticks, in order.
fn backticked(text: &str) -> Vec<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// Crate names in the first column of the table that follows the line `marker`.
fn crate_table(doc: &str, marker: &str) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    let mut lines = doc.lines().skip_while(|l| !l.starts_with(marker)).skip(1);
    let mut in_table = false;
    for line in &mut lines {
        if line.starts_with('|') {
            in_table = true;
            if let Some(name) = backticked(line).first()
                && name.starts_with("atlas-")
            {
                names.insert((*name).to_owned());
            }
        } else if in_table {
            break;
        }
    }
    names
}

fn crates_on_disk() -> BTreeSet<String> {
    fs::read_dir(root().join("crates"))
        .expect("the crates folder")
        .filter_map(Result::ok)
        .filter(|e| e.path().join("Cargo.toml").is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

#[test]
fn the_crate_tables_match_the_workspace() {
    let doc = read("documents/architecture.md");
    let built = crate_table(&doc, "Built:");
    let planned = crate_table(&doc, "Planned (");
    let on_disk = crates_on_disk();

    assert!(
        !built.is_empty() && !planned.is_empty(),
        "architecture.md must keep its \"Built:\" and \"Planned (\" crate tables"
    );
    assert_eq!(
        built, on_disk,
        "architecture.md's Built table must list exactly the crates in crates/"
    );
    let both: Vec<_> = planned.intersection(&on_disk).collect();
    assert!(
        both.is_empty(),
        "these crates now exist; move them from Planned to Built in architecture.md: {both:?}"
    );
}

#[test]
fn every_path_a_living_doc_names_exists() {
    let mut missing = Vec::new();
    for doc in LIVING_DOCS {
        for token in backticked(&read(doc)) {
            let path = token.trim_end_matches(['.', ',', ';', ':', ')']);
            let is_path =
                ROOTS.iter().any(|r| path.starts_with(r)) && !path.contains(['<', '*', ' ', '{']);
            if is_path && !root().join(path).exists() {
                missing.push(format!("{doc}: {path}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "docs name paths that don't exist (fix the doc or the path):\n{}",
        missing.join("\n")
    );
}

#[test]
fn the_strategy_page_keeps_its_sections() {
    let doc = read("documents/strategy.md");
    let missing: Vec<_> = STRATEGY_SECTIONS
        .iter()
        .filter(|s| !doc.lines().any(|l| l.starts_with(**s)))
        .collect();
    assert!(missing.is_empty(), "strategy.md lost sections: {missing:?}");
}
