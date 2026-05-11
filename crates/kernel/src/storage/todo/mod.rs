//! Todo list persistence - simple file-based storage for todo data

use crate::types::{KernelError, Result};
use async_trait::async_trait;
use serde::Deserialize;

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

// ------------------------------------------------------------------
// Shared todo types (used by both kernel and tui)
// ------------------------------------------------------------------

/// Todo item status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

/// A todo item
#[derive(Debug, Clone, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
}

/// Todo list data structure for JSON parsing
#[derive(Debug, Clone, Deserialize)]
pub struct TodoListData {
    pub todos: Vec<TodoItem>,
}

// ------------------------------------------------------------------
// System reminder helpers
// ------------------------------------------------------------------

/// Start tag for system reminders injected into user messages.
pub const SYSTEM_REMINDER_START: &str = "<system_reminder>";
/// End tag for system reminders injected into user messages.
pub const SYSTEM_REMINDER_END: &str = "</system_reminder>";

/// Strip `<system_reminder>...</system_reminder>` blocks from text.
pub fn strip_system_reminders(text: &str) -> String {
    let mut result = text.to_string();
    while let Some(start) = result.find(SYSTEM_REMINDER_START) {
        if let Some(end) = result[start..].find(SYSTEM_REMINDER_END) {
            let end_pos = start + end + SYSTEM_REMINDER_END.len();
            result.replace_range(start..end_pos, "");
        } else {
            break;
        }
    }
    result
}

pub mod json;
pub use json::JsonTodoStore;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_system_reminders() {
        let text = "Hello\n<system_reminder>\nReminder\n</system_reminder>\nWorld";
        let result = strip_system_reminders(text);
        assert!(!result.contains("<system_reminder>"));
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn test_strip_system_reminders_multiple() {
        let text = "A<system_reminder>1</system_reminder>B<system_reminder>2</system_reminder>C";
        let result = strip_system_reminders(text);
        assert_eq!(result, "ABC");
    }

    #[test]
    fn test_strip_system_reminders_unclosed() {
        let text = "Hello<system_reminder>no end";
        let result = strip_system_reminders(text);
        assert_eq!(result, "Hello<system_reminder>no end");
    }
}
