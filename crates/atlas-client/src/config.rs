//! Where the client keeps its files, its pairing (`client.toml`) and its trust state
//! (`state.json`).

use std::fmt;
use std::path::{Path, PathBuf};

use atlas_core::release::OwnerKey;
use serde::{Deserialize, Serialize};

use crate::fsx;

/// Every path the client uses, derived from the login's home folder.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    /// `~/.config/atlas/client.toml`
    pub config: PathBuf,
    /// `~/.config/atlas/cli.toml`
    pub cli: PathBuf,
    /// `~/.local/state/atlas-client/state.json`
    pub state: PathBuf,
    /// `~/.local/share/atlas/sources`: one local copy per release source.
    pub sources: PathBuf,
}

impl Paths {
    #[must_use]
    pub fn new(home: &Path) -> Self {
        Self {
            home: home.to_path_buf(),
            config: home.join(".config/atlas/client.toml"),
            cli: home.join(".config/atlas/cli.toml"),
            state: home.join(".local/state/atlas-client/state.json"),
            sources: home.join(".local/share/atlas/sources"),
        }
    }
}

/// Admin CLI credentials for managing machines on a remote hub (`~/.config/atlas/cli.toml`).
/// Kept separate from `client.toml` so ordinary sync logins stay unprivileged.
#[derive(Clone, Serialize, Deserialize)]
pub struct CliConfig {
    /// The hub's origin, such as `https://hub.example.ts.net:7443`.
    pub hub: String,
    pub token: String,
}

impl fmt::Debug for CliConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CliConfig")
            .field("hub", &self.hub)
            .field("token", &"<hidden>")
            .finish()
    }
}

impl CliConfig {
    /// # Errors
    /// The file is missing or isn't valid TOML.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            format!(
                "can't read {}: {e}. Create a CLI token in Settings → CLI tokens, then run `atlasd auth <hub-url> <token>`.",
                path.display()
            )
        })?;
        toml::from_str(&text).map_err(|e| format!("{} isn't valid: {e}", path.display()))
    }

    /// # Errors
    /// The file couldn't be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        fsx::write_private(path, text.as_bytes())
            .map_err(|e| format!("can't write {}: {e}", path.display()))
    }
}

/// What pairing gave this login. The token is a secret, so `Debug` hides it.
#[derive(Clone, Serialize, Deserialize)]
pub struct Config {
    /// The hub's origin, such as `https://atlas.example.ts.net`.
    pub hub: String,
    pub client_id: String,
    pub token: String,
    pub machine: String,
    /// Folders that older setups linked skills to (such as `~/Knowledge/skills`).
    /// Links into them count as Atlas's, so atlasd may repoint them. `~/` is the home.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adopt: Vec<String>,
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Config")
            .field("hub", &self.hub)
            .field("client_id", &self.client_id)
            .field("token", &"<hidden>")
            .field("machine", &self.machine)
            .field("adopt", &self.adopt)
            .finish()
    }
}

impl Config {
    /// The `adopt` folders as absolute paths.
    #[must_use]
    pub fn adopt_roots(&self, home: &Path) -> Vec<PathBuf> {
        self.adopt
            .iter()
            .map(|a| match a.strip_prefix("~/") {
                Some(rest) => home.join(rest),
                None => PathBuf::from(a),
            })
            .filter(|p| p.is_absolute())
            .collect()
    }

    /// # Errors
    /// The file is missing or isn't a valid config.
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| {
            format!(
                "can't read {}: {e}. Pair first with `atlasd pair <hub> <code>`.",
                path.display()
            )
        })?;
        toml::from_str(&text).map_err(|e| format!("{} isn't valid: {e}", path.display()))
    }

    /// # Errors
    /// The file couldn't be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string(self).map_err(|e| e.to_string())?;
        fsx::write_private(path, text.as_bytes())
            .map_err(|e| format!("can't write {}: {e}", path.display()))
    }
}

/// What the client trusts and has applied. Changes only after a release applied.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    /// Keys whose signatures this login accepts.
    pub owner_keys: Vec<OwnerKey>,
    pub last_applied: Option<u64>,
    /// SHA-256 (hex) of the last applied manifest, so the same release can be applied
    /// again to fix drift even after it rotated away the key that signed it.
    #[serde(default)]
    pub last_digest: Option<String>,
}

impl State {
    /// # Errors
    /// The file is missing or isn't valid.
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| {
            format!(
                "can't read {}: {e}. Pair first with `atlasd pair <hub> <code>`.",
                path.display()
            )
        })?;
        serde_json::from_slice(&bytes).map_err(|e| format!("{} isn't valid: {e}", path.display()))
    }

    /// # Errors
    /// The file couldn't be written.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        bytes.push(b'\n');
        fsx::write_private(path, &bytes).map_err(|e| format!("can't write {}: {e}", path.display()))
    }
}
