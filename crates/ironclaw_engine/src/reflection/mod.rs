//! Post-thread reflection pipeline.
//!
//! After a thread completes, [`reflect()`] uses the LLM to produce structured
//! knowledge (MemoryDocs) from the thread's execution trace:
//! - Summary — what the thread accomplished
//! - Lesson — what was learned from errors/workarounds
//! - Issue — unresolved problems for follow-up

pub mod pipeline;

pub use pipeline::{reflect, ReflectionResult};
