//! atlasd's logic, kept apart from HTTP so tests can drive it with a temp home.
//!
//! One pass: check a signed release against the keys this login trusts ([`release`]),
//! bring the local copies of its sources to the pinned trees ([`sources`]), read what
//! should be installed from those copies ([`desired`]), plan the links and files each
//! present harness needs ([`plan`]), apply the plan ([`apply`]) and describe the result
//! for the hub ([`report`]). [`pass`] strings them together. Nothing here runs content
//! it receives.

pub mod apply;
pub mod config;
pub mod desired;
mod fsx;
pub mod http;
pub mod pass;
mod place;
pub mod plan;
pub mod release;
pub mod report;
pub mod sources;

use std::path::Path;

/// Shows a path with the home folder as `~`.
#[must_use]
pub fn display(home: &Path, path: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}
