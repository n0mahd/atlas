//! Reads the store (knowledge repo, public skills repo, project repos) and the harness
//! folders of the login it runs as.
//!
//! Store reads go through cap-std, so a symlink inside a repo can't make the hub read
//! files outside that repo. Writes happen only in `edit` (one file) and `library`
//! (folders and the placements file), atomically and inside the repo.

mod catalog;
pub mod edit;
mod harness;
pub mod library;
pub mod outside;
mod paths;

pub use catalog::load_catalog;
pub use harness::{scan_harnesses, scan_projects};
pub use paths::{Project, Sources, discover_projects};
