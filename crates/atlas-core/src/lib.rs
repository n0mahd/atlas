//! Atlas domain core.
//!
//! Pure types, ports and use cases. This crate performs no I/O and depends on no async
//! runtime or web framework; `tests/dependency_allowlist.rs` enforces that.

pub mod catalog;
pub mod distribution;
pub mod kernel;
pub mod untrusted;
