//! Memory management module
//!
//! Provides project memory loading (CLAUDE.md, AGENTS.md) for system prompts.

pub mod project;

// Re-export commonly used types
pub use project::{load, MemoryFile, MemoryFiles};
