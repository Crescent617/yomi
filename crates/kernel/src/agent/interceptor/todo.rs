use super::{InterceptCtx, UserMsgInterceptor};
use crate::storage::todo::{TodoListData, TodoStatus, SYSTEM_REMINDER_END, SYSTEM_REMINDER_START};
use crate::storage::TodoStore;
use crate::types::{ContentBlock, Message, Role};
use async_trait::async_trait;
use std::sync::Arc;

/// Interceptor that appends a todo reminder every N user messages
/// **since the last todo tool was used** when there are incomplete todos.
pub struct TodoReminderInterceptor {
    todo_storage: Arc<dyn TodoStore>,
    /// Trigger every `interval` user messages since the last todo tool call
    interval: usize,
}

impl TodoReminderInterceptor {
    pub fn new(todo_storage: Arc<dyn TodoStore>) -> Self {
        Self {
            todo_storage,
            interval: 3,
        }
    }

    /// Set the reminder interval (default: 5)
    #[must_use]
    pub const fn with_interval(mut self, interval: usize) -> Self {
        self.interval = interval;
        self
    }

    async fn build_reminder(&self, session_id: &str) -> Option<String> {
        let json_str = self.todo_storage.load(session_id).await.ok()?;
        let json_str = json_str?;
        let data: TodoListData = serde_json::from_str(&json_str).ok()?;

        let mut reminder = String::new();
        for todo in data.todos {
            if !matches!(todo.status, TodoStatus::Pending | TodoStatus::InProgress) {
                continue;
            }
            if reminder.is_empty() {
                reminder.push('\n');
                reminder.push_str(SYSTEM_REMINDER_START);
                reminder.push_str("\nReminder: There are pending todos, update them using the todo tool if needed.");
            }
            let icon = match todo.status {
                TodoStatus::Pending => "(pending)",
                TodoStatus::InProgress => "(in progress)",
                // Unreachable: only Pending/InProgress items reach this point
                // (see filter above). Kept for exhaustive match.
                TodoStatus::Completed => "(completed)",
            };
            reminder.push('\n');
            reminder.push_str(&todo.id);
            reminder.push_str(". ");
            reminder.push_str(icon);
            reminder.push(' ');
            reminder.push_str(&todo.content);
        }

        if reminder.is_empty() {
            return None;
        }

        reminder.push('\n');
        reminder.push_str(SYSTEM_REMINDER_END);
        Some(reminder)
    }

    /// Count user messages since the most recent todo tool call in history.
    ///
    /// A "todo tool call" is an assistant message whose `tool_calls` contain
    /// the unified todo tool.
    fn user_msgs_since_last_todo(history: &[Arc<Message>]) -> usize {
        const TODO_TOOL: &str = crate::tools::TODO_TOOL_NAME;

        let last_todo_idx = history.iter().rposition(|msg| {
            msg.role == Role::Assistant
                && msg
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| calls.iter().any(|c| c.name == TODO_TOOL))
        });

        let start = last_todo_idx.map_or(0, |i| i + 1);
        history[start..]
            .iter()
            .filter(|msg| msg.role == Role::User)
            .count()
    }
}

#[async_trait]
impl UserMsgInterceptor for TodoReminderInterceptor {
    async fn intercept(&self, content: &mut Vec<ContentBlock>, ctx: &InterceptCtx<'_>) {
        if self.interval == 0 {
            return;
        }

        let since = Self::user_msgs_since_last_todo(ctx.history);
        // `since` counts user messages AFTER the last todo tool.
        // The current user message is not yet in history, so this call represents
        // the (since + 1)-th user message since the todo tool.
        let current = since + 1;
        if !current.is_multiple_of(self.interval) {
            return;
        }

        let Some(reminder) = self.build_reminder(ctx.session_id).await else {
            return;
        };

        // Append reminder to the last text block, or create a new one
        if let Some(ContentBlock::Text { text }) = content.last_mut() {
            text.push_str(&reminder);
        } else {
            content.push(ContentBlock::Text { text: reminder });
        }
    }
}

#[cfg(test)]
#[path = "todo_test.rs"]
mod tests;
