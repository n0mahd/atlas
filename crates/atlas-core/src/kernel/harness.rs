use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An agent harness whose config Atlas writes. Atlas never patches a harness; it only
/// writes the harness's own config files.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HarnessKind {
    ClaudeCode,
    Codex,
    OpenCode,
    Pi,
    Antigravity,
    Amp,
    Muse,
    Grok,
    T3,
    Herdr,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown harness {0:?}")]
pub struct UnknownHarness(String);

impl HarnessKind {
    pub const ALL: [Self; 10] = [
        Self::ClaudeCode,
        Self::Codex,
        Self::OpenCode,
        Self::Pi,
        Self::Antigravity,
        Self::Amp,
        Self::Muse,
        Self::Grok,
        Self::T3,
        Self::Herdr,
    ];

    /// The stable identifier used in files and URLs.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::Codex => "codex",
            Self::OpenCode => "open-code",
            Self::Pi => "pi",
            Self::Antigravity => "antigravity",
            Self::Amp => "amp",
            Self::Muse => "muse",
            Self::Grok => "grok",
            Self::T3 => "t3",
            Self::Herdr => "herdr",
        }
    }

    /// Hosts run other agents and load no skills or instructions of their own (T3 Code,
    /// Herdr), so "every agent" never includes them.
    #[must_use]
    pub const fn is_host(self) -> bool {
        matches!(self, Self::T3 | Self::Herdr)
    }

    /// Every harness that is an agent (not a host), for "every agent" choices.
    pub fn agents() -> impl Iterator<Item = Self> {
        Self::ALL.into_iter().filter(|k| !k.is_host())
    }

    /// The name shown in the UI.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
            Self::OpenCode => "OpenCode",
            Self::Pi => "Pi",
            Self::Antigravity => "Antigravity",
            Self::Amp => "Amp",
            Self::Muse => "Muse",
            Self::Grok => "Grok Build",
            Self::T3 => "T3 Code",
            Self::Herdr => "Herdr",
        }
    }
}

impl fmt::Display for HarnessKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

impl FromStr for HarnessKind {
    type Err = UnknownHarness;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|h| h.id() == s)
            .ok_or_else(|| UnknownHarness(s.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_round_trips_and_matches_serde() {
        for h in HarnessKind::ALL {
            assert_eq!(h.id().parse::<HarnessKind>().unwrap(), h);
            assert_eq!(
                serde_json::to_string(&h).unwrap(),
                format!("\"{}\"", h.id())
            );
        }
        assert!("vim".parse::<HarnessKind>().is_err());
    }
}
