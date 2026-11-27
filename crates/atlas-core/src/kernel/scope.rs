use serde::{Deserialize, Serialize};

use super::Slug;

/// Where an item applies: everywhere, or inside one registered project.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(tag = "scope", content = "project", rename_all = "kebab-case")]
pub enum Scope {
    Global,
    Project(Slug),
}

impl Scope {
    #[must_use]
    pub fn project(&self) -> Option<&Slug> {
        match self {
            Self::Global => None,
            Self::Project(slug) => Some(slug),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialises_with_a_tag() {
        let project = Scope::Project(Slug::new("atlas").unwrap());
        assert_eq!(
            serde_json::to_string(&project).unwrap(),
            r#"{"scope":"project","project":"atlas"}"#
        );
        assert_eq!(
            serde_json::to_string(&Scope::Global).unwrap(),
            r#"{"scope":"global"}"#
        );
    }
}
