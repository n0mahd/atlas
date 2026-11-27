//! The only place untrusted markdown becomes HTML.

use std::collections::HashSet;

use atlas_core::untrusted::{Markdown, Untrusted};
use pulldown_cmark::{Options, Parser, html};

/// Renders markdown and sanitises the result. `[[name]]` links become links to the
/// memory page of that name when one exists.
#[must_use]
pub fn render(source: &Untrusted<Markdown>, memory_names: &HashSet<&str>) -> String {
    let text = link_wiki_names(source.raw().as_str(), memory_names);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let mut out = String::with_capacity(text.len() * 3 / 2);
    html::push_html(&mut out, Parser::new_ext(&text, options));
    ammonia::Builder::default()
        .link_rel(Some("noopener noreferrer"))
        .url_relative(ammonia::UrlRelative::PassThrough)
        .clean(&out)
        .to_string()
}

fn link_wiki_names(text: &str, names: &HashSet<&str>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("[[") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        match after.find("]]") {
            Some(end) if linkable(after[..end].trim(), names) => {
                let name = after[..end].trim();
                out.extend(["[", name, "](/memory/", name, ")"]);
                rest = &after[end + 2..];
            }
            _ => {
                out.push_str("[[");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Only plain names become links, so a page name can't inject markdown.
fn linkable(name: &str, names: &HashSet<&str>) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && names.contains(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn md(s: &str) -> Untrusted<Markdown> {
        Untrusted::new(Markdown(s.to_owned()))
    }

    #[test]
    fn strips_scripts_and_handlers() {
        let html = render(
            &md(
                "# Hi\n<script>alert(1)</script>\n<img src=x onerror=alert(1)>\n\n[x](javascript:alert(1))",
            ),
            &HashSet::new(),
        );
        assert!(html.contains("<h1>Hi</h1>"));
        for bad in ["<script", "onerror", "href=\"javascript"] {
            assert!(!html.contains(bad), "{bad} in {html}");
        }
    }

    #[test]
    fn links_known_memory_pages_only() {
        let names = HashSet::from(["known"]);
        let html = render(&md("See [[known]] and [[unknown]]."), &names);
        assert!(
            html.contains(r#"<a href="/memory/known" rel="noopener noreferrer">known</a>"#),
            "{html}"
        );
        assert!(html.contains("[[unknown]]"));
    }
}
