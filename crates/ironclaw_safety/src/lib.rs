//! Safety layer for prompt injection defense.
//!
//! This crate re-exports the shared safety implementation from `src/safety`
//! so internal workspace users compile against the exact same source as the
//! main `ironclaw` crate.

#[path = "../../../src/safety/mod.rs"]
mod internal;

pub use internal::*;
