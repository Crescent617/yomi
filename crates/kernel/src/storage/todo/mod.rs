//! Todo list persistence - simple file-based storage for todo data

use crate::types::{KernelError, Result};
use async_trait::async_trait;

/// Storage for todo lists
#[async_trait]
pub trait TodoStore: Send + Sync {
    /// Save todo JSON for a session
    async fn save(&self, session_id: &str, json: &str) -> Result<()>;

    /// Load todo JSON for a session, returns None if not exists
    async fn load(&self, session_id: &str) -> Result<Option<String>>;

    /// Clear todos for a session
    async fn clear(&self, session_id: &str) -> Result<()>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod json;
pub use json::JsonTodoStore;
