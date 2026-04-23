use crate::types::Message;
use std::sync::Arc;

/// Simple message buffer for agent conversation history
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Arc<Message>>,
}

impl Default for MessageBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[allow(dead_code)]
impl MessageBuffer {
    /// Create an empty buffer
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
        }
    }

    /// Create from existing messages (for recovery)
    pub fn from_messages(messages: Vec<Message>) -> Self {
        Self {
            messages: messages.into_iter().map(Arc::new).collect(),
        }
    }

    /// Create from existing Arc messages (internal use)
    pub fn from_arc_messages(messages: &[Arc<Message>]) -> Self {
        Self {
            messages: messages.to_vec(),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(Arc::new(message));
    }

    /// Push an already-arc-wrapped message
    pub fn push_arc(&mut self, message: Arc<Message>) {
        self.messages.push(message);
    }

    pub fn messages(&self) -> &[Arc<Message>] {
        &self.messages
    }

    /// Get mutable access to the underlying vector (use with caution)
    pub fn messages_mut(&mut self) -> &mut Vec<Arc<Message>> {
        &mut self.messages
    }

    pub const fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Clear all messages
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Update a message using Copy-on-Write pattern
    /// If the Arc is shared, it will be cloned before modification
    pub fn update_message<F>(&mut self, idx: usize, f: F)
    where
        F: FnOnce(&mut Message),
    {
        if let Some(arc) = self.messages.get_mut(idx) {
            // Arc::make_mut will clone the inner data if it's shared
            let message = Arc::make_mut(arc);
            f(message);
        }
    }

    /// Get a clone of the messages as a new Vec<Arc<Message>>
    pub fn clone_messages(&self) -> Vec<Arc<Message>> {
        self.messages.clone()
    }

    /// Validate and clean message history before sending to provider.
    /// Removes assistant messages with `tool_calls` that don't have corresponding tool responses,
    /// and removes tool responses that are not immediately after their corresponding assistant.
    /// Time: O(n), Space: O(k) where k = number of pending tool calls
    pub fn validate_and_clean(&mut self) {
        use crate::types::Role;
        use std::collections::HashSet;

        // First pass: find all valid (assistant -> tool chain) groups
        // A tool response is valid only if it immediately follows its assistant
        let mut to_remove = HashSet::new();
        let n = self.messages.len();
        let mut i = 0;
        let mut expected_tool_ids = HashSet::new();
        let mut tool_msg_indices = Vec::new();

        while i < n {
            let msg = &self.messages[i];

            // Non-assistant: Tool gets marked, others skipped
            let Role::Assistant = msg.role else {
                if msg.role == Role::Tool {
                    to_remove.insert(i);
                }
                i += 1;
                continue;
            };

            // Assistant without tool_calls: skip
            let Some(calls) = msg.tool_calls.as_ref() else {
                i += 1;
                continue;
            };

            expected_tool_ids.clear();
            tool_msg_indices.clear();

            for call in calls {
                expected_tool_ids.insert(call.id.clone());
            }

            let tool_call_count = calls.len();
            let mut valid_chain = true;

            for tool_idx in i + 1..=i + tool_call_count {
                let Some(tool_msg) = self.messages.get(tool_idx) else {
                    valid_chain = false;
                    break;
                };

                if tool_msg.role != Role::Tool {
                    valid_chain = false;
                    break;
                }

                tool_msg_indices.push(tool_idx);

                let Some(ref tool_call_id) = tool_msg.tool_call_id else {
                    valid_chain = false;
                    break;
                };

                if !expected_tool_ids.remove(tool_call_id) {
                    valid_chain = false;
                    break;
                }
            }

            // Check if all expected tool calls have responses
            if valid_chain && !expected_tool_ids.is_empty() {
                valid_chain = false;
            }

            if !valid_chain {
                to_remove.insert(i);
                to_remove.extend(tool_msg_indices.iter());
                i += 1 + tool_msg_indices.len();
                continue;
            }

            // Valid chain - skip past all tool responses
            i += tool_call_count + 1;
        }

        if to_remove.is_empty() {
            return;
        }

        let mut i = 0;
        self.messages.retain(|_| {
            let keep = !to_remove.contains(&i);
            i += 1;
            keep
        });
    }
}

#[cfg(test)]
mod validate_clean_tests {
    use super::*;
    use crate::types::{ContentBlock, Message, Role, ToolCall};
    use chrono::Utc;

    fn create_assistant_with_tools(tool_ids: Vec<&str>) -> Message {
        Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "calling tools".to_string(),
            }],
            tool_calls: Some(
                tool_ids
                    .into_iter()
                    .map(|tid| ToolCall {
                        id: tid.to_string(),
                        name: "test_tool".to_string(),
                        arguments: serde_json::json!({}),
                    })
                    .collect(),
            ),
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    fn create_tool_response(tool_call_id: &str) -> Message {
        Message {
            role: Role::Tool,
            content: vec![ContentBlock::Text {
                text: "result".to_string(),
            }],
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    fn create_user_message(content: &str) -> Message {
        Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: content.to_string(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    #[test]
    fn test_valid_chain_kept() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_tool_response("t1"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.messages()[0].role, Role::Assistant);
        assert_eq!(buffer.messages()[1].role, Role::Tool);
    }

    #[test]
    fn test_multiple_tools_kept() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
        buffer.push(create_tool_response("t1"));
        buffer.push(create_tool_response("t2"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 3);
    }

    #[test]
    fn test_interrupted_chain_removed() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_user_message("interrupt"));
        buffer.push(create_tool_response("t1"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 1);
        assert_eq!(buffer.messages()[0].role, Role::User);
    }

    #[test]
    fn test_orphan_tool_removed() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_tool_response("t1"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_missing_tool_response_removed() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1", "t2"]));
        buffer.push(create_tool_response("t1"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_extra_tool_removed() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_tool_response("t1"));
        buffer.push(create_tool_response("extra"));

        buffer.validate_and_clean();

        // Only the orphan extra tool is removed, valid chain is kept
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.messages()[0].role, Role::Assistant);
        assert_eq!(buffer.messages()[1].role, Role::Tool);
    }

    #[test]
    fn test_wrong_tool_id_removed() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_tool_response("t2"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_multiple_valid_chains() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_tool_response("t1"));
        buffer.push(create_assistant_with_tools(vec!["t2"]));
        buffer.push(create_tool_response("t2"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 4);
    }

    #[test]
    fn test_mixed_chains() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_tool_response("t1"));
        buffer.push(create_assistant_with_tools(vec!["t2"]));
        buffer.push(create_user_message("interrupt"));
        buffer.push(create_tool_response("t2"));
        buffer.push(create_tool_response("orphan"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.messages()[0].role, Role::Assistant);
        assert_eq!(buffer.messages()[1].role, Role::Tool);
        assert_eq!(buffer.messages()[2].role, Role::User);
    }

    #[test]
    fn test_empty_buffer() {
        let mut buffer = MessageBuffer::new();
        buffer.validate_and_clean();
        assert_eq!(buffer.len(), 0);
    }

    #[test]
    fn test_assistant_without_tools() {
        let mut buffer = MessageBuffer::new();
        buffer.push(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        });
        buffer.push(create_user_message("response"));

        buffer.validate_and_clean();

        assert_eq!(buffer.len(), 2);
    }

    #[test]
    fn test_duplicate_tool_response_removed() {
        let mut buffer = MessageBuffer::new();
        buffer.push(create_assistant_with_tools(vec!["t1"]));
        buffer.push(create_tool_response("t1"));
        buffer.push(create_tool_response("t1"));

        buffer.validate_and_clean();

        // Only the duplicate tool response is removed, valid chain is kept
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.messages()[0].role, Role::Assistant);
        assert_eq!(buffer.messages()[1].role, Role::Tool);
    }
}
