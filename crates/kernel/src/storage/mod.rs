//! Storage layer - organized by functional domain
//!
//! Each domain is independent with its own trait and implementations:
//! - `session`: Session lifecycle and metadata
//! - `message`: Chat message history
//! - `usage`: Token usage tracking
//! - `todo`: Todo list persistence
//! - `file_state`: File modification tracking
//!
//! # Quick Start
//! Use [`StorageSet`] to initialize all storage backends at once:
//!
//! ```no_run
//! use std::path::PathBuf;
//! use kernel::storage::{NewSession, StorageSet};
//! use kernel::types::SessionId;
//!
//! # async fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let storage = StorageSet::open(PathBuf::from("~/.yomi")).await?;
//! let session_id = SessionId::new();
//! storage.session_store().create(NewSession::new(session_id)).await?;
//! # Ok(())
//! # }
//! ```

pub mod favorite;
pub mod file_state;
pub mod gc;
pub mod jsonl_store;
pub mod message;
pub mod pinned_session;
pub mod project;
pub mod session;
pub mod todo;
pub mod usage;

// Unified initialization
mod init;
pub use init::StorageSet;

// Migrations are internal - use StorageSet::open() instead
pub(crate) mod migrations;

// Re-export common types for convenience
pub use favorite::{AddFavoriteInput, FavoriteAnswer, FavoriteStore, SqliteFavoriteStore};
pub use file_state::{FileState, FileStateStore, JsonlFileStateStore};
pub use gc::{GarbageCollector, GcOptions, GcReport};
pub use message::{JsonlMessageStore, MessageStore};
pub use pinned_session::{
    PinnedSessionDetail, PinnedSessionInfo, PinnedSessionStore, SqlitePinnedSessionStore,
};
pub use project::{ProjectStore, SqliteProjectStore};
pub use session::{
    format_age, NewSession, SessionInfo, SessionOverrides, SessionStore, SqliteSessionStore,
};
pub use todo::{
    strip_system_reminders, JsonTodoStore, TodoItem, TodoListData, TodoStatus, TodoStore,
    SYSTEM_REMINDER_END, SYSTEM_REMINDER_START,
};
pub use usage::{SqliteUsageStore, UsageRecord, UsageStore, UsageSummary, UsageType};

/// Shared helper for constructing `Storage` errors.
pub(crate) fn storage_err(msg: impl Into<String>) -> crate::types::KernelError {
    crate::types::KernelError::Storage(msg.into())
}
