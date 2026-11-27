/// The YAML block at the top of a markdown file, still unparsed. The filesystem adapter
/// parses it into the shapes this module defines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frontmatter<'a>(pub &'a str);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrontmatterError {
    #[error("the file starts a frontmatter block (---) but never closes it")]
    Unclosed,
}

/// Splits `---\n<yaml>\n---\n<body>` into its frontmatter and body. A file that doesn't
/// start with `---` has no frontmatter and is all body.
///
/// # Errors
/// [`FrontmatterError::Unclosed`] if the opening `---` has no closing line.
pub fn split_frontmatter(text: &str) -> Result<(Option<Frontmatter<'_>>, &str), FrontmatterError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let Some(rest) = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))
    else {
        return Ok((None, text));
    };
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            let body = &rest[offset + line.len()..];
            return Ok((Some(Frontmatter(&rest[..offset])), body));
        }
        offset += line.len();
    }
    Err(FrontmatterError::Unclosed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_frontmatter_from_body() {
        let (fm, body) = split_frontmatter("---\nname: a\n---\n# Title\n").unwrap();
        assert_eq!(fm, Some(Frontmatter("name: a\n")));
        assert_eq!(body, "# Title\n");
    }

    #[test]
    fn handles_crlf_bom_and_empty_blocks() {
        let (fm, body) = split_frontmatter("\u{feff}---\r\nname: a\r\n---\r\nbody").unwrap();
        assert_eq!(fm, Some(Frontmatter("name: a\r\n")));
        assert_eq!(body, "body");
        let (fm, body) = split_frontmatter("---\n---\n").unwrap();
        assert_eq!(fm, Some(Frontmatter("")));
        assert_eq!(body, "");
    }

    #[test]
    fn no_frontmatter_is_all_body() {
        assert_eq!(
            split_frontmatter("# Hi\n---\n").unwrap(),
            (None, "# Hi\n---\n")
        );
    }

    #[test]
    fn unclosed_block_is_an_error() {
        assert_eq!(
            split_frontmatter("---\nname: a\n"),
            Err(FrontmatterError::Unclosed)
        );
    }

    #[test]
    fn a_dashed_line_inside_yaml_text_still_closes() {
        let (fm, body) = split_frontmatter("---\na: 1\n---\nb\n---\n").unwrap();
        assert_eq!(fm, Some(Frontmatter("a: 1\n")));
        assert_eq!(body, "b\n---\n");
    }
}
