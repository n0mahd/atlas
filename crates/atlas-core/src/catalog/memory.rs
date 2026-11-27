use serde::Deserialize;

use crate::untrusted::{Markdown, Untrusted};

/// The kind of fact a memory page holds, from its `metadata.type`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MemoryKind {
    User,
    Feedback,
    Project,
    Reference,
    /// Missing or not one of the four: shown as needing a look.
    Unknown(String),
}

impl MemoryKind {
    #[must_use]
    pub fn parse(value: Option<&str>) -> Self {
        match value.map(str::trim) {
            Some("user") => Self::User,
            Some("feedback") => Self::Feedback,
            Some("project") => Self::Project,
            Some("reference") => Self::Reference,
            other => Self::Unknown(other.unwrap_or_default().to_owned()),
        }
    }

    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::User => "user",
            Self::Feedback => "feedback",
            Self::Project => "project",
            Self::Reference => "reference",
            Self::Unknown(_) => "untyped",
        }
    }
}

/// One memory page: a single durable fact.
#[derive(Clone, Debug)]
pub struct MemoryPage {
    /// The file stem, which is how pages link to each other (`[[name]]`).
    pub name: String,
    pub description: String,
    pub kind: MemoryKind,
    /// The machine this fact is about, or `all`.
    pub host: String,
    /// Path inside the memory folder, for display.
    pub path: String,
    /// Other pages this one links to with `[[name]]`.
    pub links: Vec<String>,
    pub body: Untrusted<Markdown>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MemoryFrontmatter {
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub metadata: MemoryMetadata,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MemoryMetadata {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub host: Option<String>,
}

/// Finds `[[name]]` links in a page body, in order, without duplicates.
#[must_use]
pub fn wiki_links(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut rest = body;
    while let Some(start) = rest.find("[[") {
        rest = &rest[start + 2..];
        let Some(end) = rest.find("]]") else { break };
        let name = rest[..end].trim();
        if !name.is_empty() && !name.contains('\n') && seen.insert(name) {
            links.push(name.to_owned());
        }
        rest = &rest[end + 2..];
    }
    links
}

impl MemoryPage {
    /// Builds a page from its file stem, display path, parsed frontmatter and body.
    #[must_use]
    pub fn new(name: String, path: String, fm: MemoryFrontmatter, body: &str) -> Self {
        Self {
            name,
            description: fm.description,
            kind: MemoryKind::parse(fm.metadata.kind.as_deref()),
            host: fm.metadata.host.unwrap_or_else(|| "all".to_owned()),
            path,
            links: wiki_links(body),
            body: Untrusted::new(Markdown(body.to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_wiki_links_once_each() {
        assert_eq!(
            wiki_links("See [[a]] and [[ b ]], again [[a]]. Broken [[c"),
            vec!["a", "b"]
        );
    }

    #[test]
    fn unknown_kinds_are_kept() {
        assert_eq!(MemoryKind::parse(Some("project")), MemoryKind::Project);
        assert_eq!(MemoryKind::parse(None).label(), "untyped");
        assert_eq!(
            MemoryKind::parse(Some("note")),
            MemoryKind::Unknown("note".into())
        );
    }
}
