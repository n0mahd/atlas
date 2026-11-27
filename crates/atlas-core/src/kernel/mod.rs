//! The shared kernel: the few types every context uses. Keep it small.

mod harness;
mod scope;
mod slug;
mod tree_hash;

pub use harness::{HarnessKind, UnknownHarness};
pub use scope::Scope;
pub use slug::{InvalidSlug, Slug};
pub use tree_hash::{InvalidTreeHash, TreeHash};
