//! atlas-core must stay pure: no I/O, async runtime or web framework. This test walks
//! the crate's normal and build dependency graph (dev-dependencies excluded) and fails
//! if any package outside the allowlist appears.

use std::collections::BTreeSet;
use std::process::Command;

use serde_json::Value;

/// serde and thiserror, plus what they pull in. Add a hashing crate here when one is
/// needed. Anything else needs a decision in `documents/decisions.md` first.
const ALLOWED: &[&str] = &[
    "atlas-core",
    "proc-macro2",
    "quote",
    "serde",
    "serde_core",
    "serde_derive",
    "syn",
    "thiserror",
    "thiserror-impl",
    "unicode-ident",
];

#[test]
fn core_depends_only_on_allowlisted_crates() {
    let output = Command::new(env!("CARGO"))
        .args(["metadata", "--format-version", "1", "--locked"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("run cargo metadata");
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let meta: Value = serde_json::from_slice(&output.stdout).expect("parse cargo metadata");

    let name_of = |id: &str| -> String {
        meta["packages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == id)
            .map(|p| p["name"].as_str().unwrap().to_owned())
            .unwrap()
    };
    let nodes = meta["resolve"]["nodes"].as_array().unwrap();
    let root = meta["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "atlas-core")
        .map(|p| p["id"].as_str().unwrap().to_owned())
        .unwrap();

    let mut seen = BTreeSet::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = nodes.iter().find(|n| n["id"] == id.as_str()).unwrap();
        for dep in node["deps"].as_array().unwrap() {
            let shipped = dep["dep_kinds"]
                .as_array()
                .unwrap()
                .iter()
                .any(|k| k["kind"] != "dev");
            if shipped {
                stack.push(dep["pkg"].as_str().unwrap().to_owned());
            }
        }
    }

    let unexpected: Vec<String> = seen
        .iter()
        .map(|id| name_of(id))
        .filter(|name| !ALLOWED.contains(&name.as_str()))
        .collect();
    assert!(
        unexpected.is_empty(),
        "atlas-core gained dependencies outside its allowlist: {unexpected:?}"
    );
}
