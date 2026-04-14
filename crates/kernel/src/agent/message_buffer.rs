use crate::types::Message;

/// Simple message buffer for agent conversation history
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Message>,
}

impl MessageBuffer {
    /// Create from existing messages (for recovery)
    pub const fn from_messages(messages: Vec<Message>) -> Self {
        Self { messages }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub const fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    /// Validate and clean message history before sending to provider.
    /// Removes assistant messages with tool_calls that don't have corresponding tool responses.
    /// Time: O(n), Space: O(k) where k = number of pending tool calls
    pub fn validate_and_clean(&mut self) {
        use crate::types::Role;
        use std::collections::HashSet;

        let mut pending_tool_calls: HashSet<String> = HashSet::new();

        // Single pass: track unmatched tool calls
        // O(n) where n = number of messages
        for msg in &self.messages {
            match msg.role {
                Role::Assistant => {
                    if let Some(ref calls) = msg.tool_calls {
                        for call in calls {
                            pending_tool_calls.insert(call.id.clone());
                        }
                    }
                }
                Role::Tool => {
                    if let Some(ref tool_call_id) = msg.tool_call_id {
                        pending_tool_calls.remove(tool_call_id);
                    }
                }
                _ => {}
            }
        }

        if pending_tool_calls.is_empty() {
            return; // All tool calls have responses, O(1) early exit
        }

        tracing::warn!(
            "Message buffer has {} pending tool calls without responses, cleaning up",
            pending_tool_calls.len()
        );

        // Single pass removal using retain: O(n)
        // retain is more efficient than individual remove() which is O(n) per operation
        self.messages.retain(|msg| {
            if let Role::Assistant = msg.role {
                if let Some(ref calls) = msg.tool_calls {
                    // Check if any tool_call in this message is pending
                    let has_pending = calls.iter()
                        .any(|call| pending_tool_calls.contains(&call.id));
                    if has_pending {
                        tracing::debug!("Removing assistant message with unmatched tool_calls: {:?}",
                            calls.iter().map(|c| &c.id).collect::<Vec<_>>());
                        return false; // Remove this message
                    }
                }
            }
            true // Keep this message
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Role};

    fn create_message(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: chrono::Utc::now(),
            token_usage: None,
        }
    }

    #[test]
    fn test_buffer_basic() {
        let mut buffer = MessageBuffer {
            messages: Vec::new(),
        };

        buffer.push(create_message(Role::System, "System prompt"));
        buffer.push(create_message(Role::User, "Message 1"));
        buffer.push(create_message(Role::Assistant, "Response 1"));

        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn test_from_messages() {
        let messages = vec![
            create_message(Role::System, "System"),
            create_message(Role::User, "User"),
        ];
        let buffer = MessageBuffer::from_messages(messages);

        assert_eq!(buffer.len(), 2);
        assert!(buffer
            .messages()
            .iter()
            .any(|m| { matches!(m.role, Role::System) }));
    }

    // ========== validate_and_clean tests ==========

    fn create_message_with_tool_calls(
        role: Role,
        text: &str,
        tool_calls: Option<Vec<crate::types::ToolCall>>,
        tool_call_id: Option<String>,
    ) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            tool_calls,
            tool_call_id,
            created_at: chrono::Utc::now(),
            token_usage: None,
        }
    }

    #[test]
    fn test_validate_and_clean_no_pending_tools() {
        // Normal conversation without tool calls - should remain unchanged
        let messages = vec![
            create_message(Role::System, "System"),
            create_message(Role::User, "Hello"),
            create_message(Role::Assistant, "Hi there!"),
        ];
        let mut buffer = MessageBuffer::from_messages(messages);

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 3);
        assert!(matches!(buffer.messages()[0].role, Role::System));
        assert!(matches!(buffer.messages()[1].role, Role::User));
        assert!(matches!(buffer.messages()[2].role, Role::Assistant));
    }

    #[test]
    fn test_validate_and_clean_with_complete_tool_calls() {
        // Complete tool call flow - assistant with tool_calls followed by tool response
        let tool_call = crate::types::ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "test.txt"}),
        };

        let messages = vec![
            create_message(Role::User, "Read the file"),
            create_message_with_tool_calls(
                Role::Assistant,
                "I'll read it",
                Some(vec![tool_call.clone()]),
                None,
            ),
            create_message_with_tool_calls(
                Role::Tool,
                "file content",
                None,
                Some("call_1".to_string()),
            ),
            create_message(Role::Assistant, "Done!"),
        ];
        let mut buffer = MessageBuffer::from_messages(messages);

        buffer.validate_and_clean();

        // All messages should be preserved (complete tool call flow)
        assert_eq!(buffer.len(), 4);
    }

    #[test]
    fn test_validate_and_clean_removes_unmatched_tool_calls() {
        // Assistant with tool_calls but no tool response - should be removed
        let tool_call = crate::types::ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "test.txt"}),
        };

        let messages = vec![
            create_message(Role::User, "Read the file"),
            create_message_with_tool_calls(
                Role::Assistant,
                "I'll read it",
                Some(vec![tool_call]),
                None,
            ),
            // No tool response!
        ];
        let mut buffer = MessageBuffer::from_messages(messages);

        buffer.validate_and_clean();

        // Assistant message with unmatched tool_calls should be removed
        assert_eq!(buffer.len(), 1);
        assert!(matches!(buffer.messages()[0].role, Role::User));
    }

    #[test]
    fn test_validate_and_clean_partial_match() {
        // Multiple tool calls, one matched, one not
        let tool_call_1 = crate::types::ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "a.txt"}),
        };
        let tool_call_2 = crate::types::ToolCall {
            id: "call_2".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "b.txt"}),
        };

        let messages = vec![
            create_message(Role::User, "Read files"),
            create_message_with_tool_calls(
                Role::Assistant,
                "I'll read them",
                Some(vec![tool_call_1.clone(), tool_call_2.clone()]),
                None,
            ),
            // Only respond to call_1
            create_message_with_tool_calls(
                Role::Tool,
                "content of a",
                None,
                Some("call_1".to_string()),
            ),
        ];
        let mut buffer = MessageBuffer::from_messages(messages);

        buffer.validate_and_clean();

        // Assistant message has unmatched call_2, should be removed
        assert_eq!(buffer.len(), 2);
        assert!(matches!(buffer.messages()[0].role, Role::User));
        assert!(matches!(buffer.messages()[1].role, Role::Tool));
    }

    #[test]
    fn test_validate_and_clean_preserves_order() {
        // Ensure messages after the removed one maintain order
        let tool_call = crate::types::ToolCall {
            id: "call_1".to_string(),
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "ls"}),
        };

        let messages = vec![
            create_message(Role::System, "System"),
            create_message(Role::User, "List files"),
            create_message_with_tool_calls(
                Role::Assistant,
                "Running command",
                Some(vec![tool_call]),
                None,
            ),
            create_message(Role::User, "What happened?"),
            create_message(Role::Assistant, "Command failed"),
        ];
        let mut buffer = MessageBuffer::from_messages(messages);

        buffer.validate_and_clean();

        // Check order is preserved after removal
        assert_eq!(buffer.len(), 4);
        assert!(matches!(buffer.messages()[0].role, Role::System));
        assert!(matches!(buffer.messages()[1].role, Role::User));
        assert!(matches!(buffer.messages()[2].role, Role::User));
        assert_eq!(
            buffer.messages()[2].content[0],
            ContentBlock::Text {
                text: "What happened?".to_string()
            }
        );
        assert!(matches!(buffer.messages()[3].role, Role::Assistant));
    }

    #[test]
    fn test_validate_and_clean_multiple_assistant_with_tools() {
        // Multiple assistant messages, some with unmatched tools
        let tool_call_1 = crate::types::ToolCall {
            id: "call_1".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "file1.txt"}),
        };
        let tool_call_2 = crate::types::ToolCall {
            id: "call_2".to_string(),
            name: "read".to_string(),
            arguments: serde_json::json!({"path": "file2.txt"}),
        };

        let messages = vec![
            create_message(Role::User, "Read files"),
            // First assistant - complete
            create_message_with_tool_calls(
                Role::Assistant,
                "Reading file1",
                Some(vec![tool_call_1.clone()]),
                None,
            ),
            create_message_with_tool_calls(
                Role::Tool,
                "content1",
                None,
                Some("call_1".to_string()),
            ),
            // Second assistant - incomplete
            create_message_with_tool_calls(
                Role::Assistant,
                "Reading file2",
                Some(vec![tool_call_2]),
                None,
            ),
            // No tool response for call_2!
        ];
        let mut buffer = MessageBuffer::from_messages(messages);

        buffer.validate_and_clean();

        // Should keep: User, Assistant1, Tool1
        // Should remove: Assistant2
        assert_eq!(buffer.len(), 3);
        assert!(matches!(buffer.messages()[1].role, Role::Assistant));
        assert!(matches!(buffer.messages()[2].role, Role::Tool));
    }
}
