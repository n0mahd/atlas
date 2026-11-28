//! The stylesheet's blocks must close. A lost `}` after a `@media` block (easy to drop
//! when merging) puts every later rule inside a phone-only query, so desktop pages lose
//! their styling while every other test passes.

/// The first problem with `css`'s braces, if any: unbalanced, or a `@media` opened
/// inside another block (atlas.css keeps every media query at the top level).
fn brace_problem(css: &str) -> Option<String> {
    let mut depth = 0_i32;
    let mut line = 1;
    let mut rest = css;
    while let Some(c) = rest.chars().next() {
        if rest.starts_with("/*") {
            let end = rest.find("*/").map_or(rest.len(), |i| i + 2);
            line += rest[..end].matches('\n').count();
            rest = &rest[end..];
            continue;
        }
        if rest.starts_with("@media") && depth != 0 {
            return Some(format!(
                "line {line}: @media opens inside a block (depth {depth})"
            ));
        }
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth < 0 {
                    return Some(format!("line {line}: a `}}` closes nothing"));
                }
            }
            '\n' => line += 1,
            _ => {}
        }
        rest = &rest[c.len_utf8()..];
    }
    (depth != 0).then(|| format!("{depth} block(s) never closed"))
}

#[test]
fn atlas_css_blocks_all_close() {
    let css = include_str!("../static/atlas.css");
    assert_eq!(brace_problem(css), None);
}

#[test]
fn the_check_catches_a_lost_brace() {
    let lost = "a { b: c; }\n@media (max-width: 720px) {\n  a { b: d; }\n\n.later { e: f; }\n";
    assert!(brace_problem(lost).is_some());
    let nested = "@media (x) {\n  a { b: c; }\n@media (y) { d { e: f; } }\n}\n";
    assert!(brace_problem(nested).unwrap().contains("inside a block"));
    assert_eq!(brace_problem("/* { */ a { b: c; }\n"), None);
}
