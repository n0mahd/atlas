//! Every route the hub serves must appear in the verification skill's feature map
//! (`.agents/skills/verify/features/*.md`, as `` `METHOD /path` ``), so a new screen or
//! endpoint can't ship without an end-to-end journey planned for it. Listing a route that
//! no longer exists fails too, so the map can't go stale.

use std::collections::BTreeSet;
use std::path::Path;

/// `(METHOD, path)` for each `.route(...)` in the router.
fn routes(src: &str) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    let mut rest = src;
    while let Some(at) = rest.find(".route(") {
        rest = &rest[at + ".route(".len()..];
        // The call runs to the matching close paren.
        let mut depth = 1;
        let end = rest
            .char_indices()
            .find_map(|(i, c)| {
                match c {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                (depth == 0).then_some(i)
            })
            .expect("unbalanced .route(");
        let call = &rest[..end];
        let path = call
            .split('"')
            .nth(1)
            .expect("route without a path literal")
            .to_owned();
        for method in ["get", "post", "put", "patch", "delete"] {
            let used = call.match_indices(&format!("{method}(")).any(|(i, _)| {
                i == 0
                    || !call.as_bytes()[i - 1].is_ascii_alphanumeric()
                        && call.as_bytes()[i - 1] != b'_'
            });
            if used {
                out.insert((method.to_ascii_uppercase(), path.clone()));
            }
        }
    }
    out
}

/// Every `` `METHOD /path` `` in the feature map.
fn mapped(dir: &Path) -> BTreeSet<(String, String)> {
    let mut out = BTreeSet::new();
    for entry in std::fs::read_dir(dir).expect("feature map folder") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for span in text.split('`').skip(1).step_by(2) {
            if let Some((method, route)) = span.split_once(' ')
                && ["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method)
                && route.starts_with('/')
                && !route.contains(' ')
            {
                out.insert((method.to_owned(), route.to_owned()));
            }
        }
    }
    out
}

#[test]
fn every_route_has_a_journey_in_the_feature_map() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    let served = routes(&src);
    assert!(served.len() > 30, "parsed too few routes: {served:?}");
    let map = mapped(&root.join("../../.agents/skills/verify/features"));

    let missing: Vec<_> = served.difference(&map).collect();
    let stale: Vec<_> = map.difference(&served).collect();
    assert!(
        missing.is_empty(),
        "routes with no feature in .agents/skills/verify/features (add a journey and list them):\n{missing:#?}"
    );
    assert!(
        stale.is_empty(),
        "feature map lists routes the hub no longer serves:\n{stale:#?}"
    );
}
