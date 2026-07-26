//! Message history storage - persistent chat message storage

use crate::types::{KernelError, Message, Result};
use async_trait::async_trait;

/// Storage for session message history
#[async_trait]
pub trait MessageStore: Send + Sync {
    /// Append messages to a session's history
    async fn append(&self, session_id: &str, messages: &[Message]) -> Result<()>;

    /// Get all messages for a session. `asset://` image references are kept
    /// as-is; frontends resolve them lazily via `read_asset`.
    async fn get(&self, session_id: &str) -> Result<Vec<Message>>;

    /// Get all messages with `asset://` images inlined as base64 data URLs,
    /// for building model context. Missing or oversized assets degrade to
    /// text placeholders rather than failing the whole load.
    async fn get_inlined(&self, session_id: &str) -> Result<Vec<Message>>;

    /// Replace all messages for a session (used by compactor)
    async fn replace(&self, session_id: &str, messages: &[Message]) -> Result<()>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod jsonl;
pub use jsonl::JsonlMessageStore;
